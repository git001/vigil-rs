//! The streaming loop shared by `source_url` and `source_unix`.

use std::time::Duration;

use tokio::io::{AsyncRead, BufReader};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{info, warn};
use vigil_types::lines::LineReader;

use super::keyed::{Keyed, Out};
use super::{SourceConnConfig, queue_payload, sse_payload};
use crate::LineFilter;
use crate::cli::record_limit;

/// Read lines from `reader` and forward them until EOF, a read error, or
/// an idle-timeout expiry (`conn.idle_timeout_ms`, 0 disables it).
///
/// Invalid UTF-8 becomes U+FFFD. A line longer than the record limit
/// ([`record_limit`] of `--max-line-bytes`) is forwarded in parts, with a
/// warning: the parts of a JSON record are no valid JSON, but nothing is
/// lost. With `--multiline*` the lines of a stack trace become one record
/// (per service and stream, see [`Keyed`]). Returns `true` on a clean EOF
/// and `false` on any unclean exit (error or timeout), so the caller knows
/// whether to increment the failure counter. `source_label` names the
/// source in log messages (the URL or socket path).
pub async fn stream_loop<R>(
    reader: R,
    tx: &mpsc::Sender<String>,
    filter: &LineFilter,
    conn: &SourceConnConfig,
    source_label: &str,
) -> bool
where
    R: AsyncRead + Unpin,
{
    let max_record = record_limit(conn.join.max);
    let mut lines = LineReader::new(BufReader::new(reader), max_record);
    let mut keyed = Keyed::new(conn.join.clone());
    let mut sender = Sender {
        tx,
        filter,
        allow_parts: false,
    };
    let idle = (conn.idle_timeout_ms > 0).then(|| Duration::from_millis(conn.idle_timeout_ms));
    // Inside a line that comes in parts: warn once per line.
    let mut splitting = false;
    loop {
        // None: the idle timeout expired.
        let read = async {
            match idle {
                Some(d) => tokio::time::timeout(d, lines.next_line()).await.ok(),
                None => Some(lines.next_line().await),
            }
        };
        let next = match keyed.deadline() {
            // next_line() is cancel safe.
            Some(deadline) => tokio::select! {
                r = read => r,
                () = tokio::time::sleep_until(deadline) => {
                    sender.send_all(keyed.flush(Some(Instant::now())));
                    continue;
                }
            },
            None => read.await,
        };
        let line = match next {
            Some(Ok(Some(line))) => line,
            end => {
                sender.send_all(keyed.flush(None));
                match end {
                    None => warn!(source = %source_label, "idle timeout — reconnecting"),
                    Some(Err(e)) => {
                        warn!(source = %source_label, error = %e, "read error — reconnecting")
                    }
                    Some(Ok(_)) => {
                        info!(source = %source_label, "stream EOF — reconnecting");
                        return true;
                    }
                }
                return false;
            }
        };
        if line.continued && !splitting {
            warn!(source = %source_label, max_bytes = max_record, "forwarding a line longer than the limit in parts (raise --max-line-bytes)");
        }
        splitting = line.continued;
        // The later parts of a split line carry no SSE framing of their own.
        let payload = if line.continued {
            Some(line.text)
        } else {
            sse_payload(line.text)
        };
        if let Some(payload) = payload
            && let Some(out) = keyed.push(payload, line.continued, Instant::now())
        {
            sender.send(out);
        }
    }
}

/// Filters and sends the complete entries of a stream.
struct Sender<'a> {
    tx: &'a mpsc::Sender<String>,
    filter: &'a LineFilter,
    /// The filter's decision on the entry whose later parts follow.
    allow_parts: bool,
}

impl Sender<'_> {
    /// The later parts of a split entry follow the decision on its first.
    fn send(&mut self, (payload, continued): Out) {
        if !continued {
            self.allow_parts = self.filter.allow(&payload);
        }
        if self.allow_parts {
            queue_payload(payload, self.tx);
        }
    }

    fn send_all(&mut self, outs: Vec<Out>) {
        for out in outs {
            self.send(out);
        }
    }
}
