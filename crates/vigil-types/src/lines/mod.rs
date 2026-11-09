// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Lines from a byte stream that may contain anything: the output of a
//! service, a pod's log, a log stream from another program.
//!
//! tokio's `lines()` fits none of them: it ends with an error at the first
//! byte that is not UTF-8 (the reader then stops, and a service writing to
//! the closed pipe dies of SIGPIPE), and it collects a line without a
//! newline for as long as the stream sends one, in memory. [`LineReader`]
//! instead
//! - turns invalid UTF-8 into U+FFFD (`�`) and goes on;
//! - bounds a line to `max` bytes: a longer one comes in parts, cut
//!   between characters, nothing is lost.
//!
//! A line ends at `\n`; a `\r` before it is removed, as by `lines()`.
//! [`EntryReader`] adds the joining of multi-line entries (stack traces)
//! on top, see [`Multiline`].

use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

mod entry;
mod multiline;

pub use entry::EntryReader;
pub use multiline::{
    DEFAULT_MULTILINE_TIMEOUT, MAX_MULTILINE_TIMEOUT, MIN_MULTILINE_TIMEOUT, Multiline, Rule,
    parse_timeout,
};

/// Default for the longest log line (or multi-line entry) kept as one
/// entry: room for a stack trace written as one JSON line.
pub const DEFAULT_MAX_LINE: usize = 128 * 1024;

/// One line, without its line ending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    /// This is a later part of a line longer than the limit.
    pub continued: bool,
}

/// Reads [`Line`]s from `R`; see the module documentation.
pub struct LineReader<R> {
    reader: R,
    max: usize,
    buf: Vec<u8>,
    /// Bytes of a character cut by a split, for the next part.
    carry: Vec<u8>,
    /// The next line continues a split one.
    continues: bool,
}

impl<R: AsyncBufRead + Unpin> LineReader<R> {
    /// Lines of at most `max` bytes; a longer line comes in parts, the
    /// parts after the first have [`Line::continued`] set. `max` must be at
    /// least 4 (one character).
    pub fn new(reader: R, max: usize) -> Self {
        assert!(max >= 4, "max must hold one character");
        LineReader {
            reader,
            max,
            buf: Vec::new(),
            carry: Vec::new(),
            continues: false,
        }
    }

    /// The next line, or `None` at the end of the stream. A last line
    /// without a newline is returned too. Errors are those of the reader.
    ///
    /// Cancel safe: a call dropped before it finished loses nothing, the
    /// next one goes on where it stopped.
    pub async fn next_line(&mut self) -> io::Result<Option<Line>> {
        // `buf` holds what a cancelled call had read so far.
        self.buf.append(&mut self.carry);
        loop {
            let avail = self.reader.fill_buf().await?;
            if avail.is_empty() {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(self.finish(false)));
            }
            let newline = avail.iter().position(|&b| b == b'\n');
            let room = self.max - self.buf.len();
            if let Some(i) = newline.filter(|&i| i <= room) {
                self.buf.extend_from_slice(&avail[..i]);
                self.reader.consume(i + 1);
                if self.buf.last() == Some(&b'\r') {
                    self.buf.pop();
                }
                return Ok(Some(self.finish(false)));
            }
            if room == 0 {
                // Full, and the next byte is not the newline: too long.
                return Ok(Some(self.cut()));
            }
            let n = avail.len().min(room);
            self.buf.extend_from_slice(&avail[..n]);
            self.reader.consume(n);
        }
    }

    /// A full buffer as one part of a split line. The bytes of a character
    /// that the limit cuts go to the next part.
    fn cut(&mut self) -> Line {
        let keep = self.buf.len() - incomplete_tail(&self.buf);
        if keep > 0 {
            self.carry = self.buf.split_off(keep);
        }
        self.finish(true)
    }

    /// The buffer as a line; `split` when more of it follows.
    fn finish(&mut self, split: bool) -> Line {
        let bytes = std::mem::take(&mut self.buf);
        let text = String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        let continued = std::mem::replace(&mut self.continues, split);
        Line { text, continued }
    }
}

/// How many bytes at the end of `b` start a character that is not complete
/// (0 to 3).
fn incomplete_tail(b: &[u8]) -> usize {
    for back in 1..=b.len().min(4) {
        let byte = b[b.len() - back];
        if byte & 0xC0 == 0x80 {
            continue; // continuation byte
        }
        let len = match byte {
            0xF0.. => 4,
            0xE0.. => 3,
            0xC0.. => 2,
            _ => 1,
        };
        return if len > back { back } else { 0 };
    }
    0
}

#[cfg(test)]
mod tests;
