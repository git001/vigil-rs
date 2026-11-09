//! Reading one log stream of a pod container and forwarding its lines.

use std::sync::Arc;

use tokio::io::{AsyncRead, BufReader};
use tokio::time::Instant;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{info, warn};
use vigil_types::lines::LineReader;

use super::records::{Dedup, Records};
use super::stream::StreamCtx;
use crate::join::{JoinConfig, Joiner};

/// How a log stream ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum StreamEnd {
    /// EOF, an error or a failed request; the reconcile loop restarts it.
    Closed,
    /// The API server does not accept the `stream` query parameter.
    StreamParamRejected,
}

/// `stream=Stdout|Stderr` needs the PodLogsQuerySplitStreams feature gate,
/// which is off by default even on recent Kubernetes versions; the API
/// server then answers 422 "stream: Forbidden: may not be specified".
fn is_stream_param_rejected(e: &kube::Error) -> bool {
    matches!(e, kube::Error::Api(r) if r.code == 422 && r.message.contains("stream: Forbidden"))
}

// ---------------------------------------------------------------------------
// Per-pod streaming task
// ---------------------------------------------------------------------------

pub(super) async fn stream_pod(
    ctx: StreamCtx,
    pod: String,
    container: Option<Arc<String>>,
    url: String,
    stream_name: &'static str,
) -> StreamEnd {
    let StreamCtx {
        client,
        namespace,
        filter,
        tx,
        lines: line_config,
        ..
    } = ctx;
    let container_str = container.as_deref().map(|s| s.as_str());

    let req = match http::Request::get(&url).body(vec![]) {
        Ok(r) => r,
        Err(e) => {
            warn!(pod = %pod, stream = stream_name, error = %e, "failed to build log request");
            return StreamEnd::Closed;
        }
    };

    let byte_stream = match client.request_stream(req).await {
        Ok(s) => s,
        Err(e) if is_stream_param_rejected(&e) => return StreamEnd::StreamParamRejected,
        Err(e) => {
            warn!(pod = %pod, stream = stream_name, error = %e, "failed to open log stream");
            return StreamEnd::Closed;
        }
    };

    let mut records = Records {
        namespace: &namespace,
        pod: &pod,
        container: container_str,
        stream: stream_name,
        filter: &filter,
        tx: &tx,
        allow_parts: false,
    };
    match forward_lines(byte_stream.compat(), &line_config, &mut records).await {
        Ok(()) => info!(pod = %pod, stream = stream_name, "log stream EOF"),
        Err(e) => warn!(pod = %pod, stream = stream_name, error = %e, "log stream error"),
    }
    StreamEnd::Closed
}

/// Read the lines of a pod's log stream ("<timestamp> <message>", as the
/// kubelet sends them with `timestamps=true`) and send them as records
/// until the stream ends; an entry still pending is sent first.
///
/// Invalid UTF-8 becomes U+FFFD; a long line comes in parts (the limit
/// leaves room for the timestamp in front of the message). Repeated lines
/// are skipped ([`Dedup`]), the lines of a stack trace joined
/// (`--multiline*`), and the filter decides on the whole record.
pub(super) async fn forward_lines(
    reader: impl AsyncRead + Unpin,
    config: &JoinConfig,
    records: &mut Records<'_>,
) -> std::io::Result<()> {
    let mut lines = LineReader::new(BufReader::new(reader), config.max + TIMESTAMP_ROOM);
    let mut dedup = Dedup::default();
    let mut joiner: Joiner<String> = config.joiner();
    loop {
        // A pending multi-line entry is complete when no line comes in
        // time; next_line() is cancel safe.
        let read = match joiner.deadline() {
            Some(deadline) => tokio::select! {
                r = lines.next_line() => r,
                () = tokio::time::sleep_until(deadline) => {
                    if let Some((ts, entry)) = joiner.flush() {
                        records.send(&ts, &entry);
                    }
                    continue;
                }
            },
            None => lines.next_line().await,
        };
        match read {
            Ok(Some(line)) => {
                let Some((ts, msg)) = dedup.accept(line) else {
                    continue;
                };
                if let Some((ts, entry)) = joiner.push(ts, msg, Instant::now()) {
                    records.send(&ts, &entry);
                }
            }
            end => {
                if let Some((ts, entry)) = joiner.flush() {
                    records.send(&ts, &entry);
                }
                return end.map(|_| ());
            }
        }
    }
}

/// Room for the timestamp and the space in front of a pod's log line
/// (`2026-03-18T11:23:45.123456789Z `, 31 bytes, or longer with an offset).
const TIMESTAMP_ROOM: usize = 64;

#[cfg(test)]
mod tests;
