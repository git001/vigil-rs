//! Reading one log stream of a pod container and forwarding its lines.

use std::sync::Arc;

use serde::Serialize;
use tokio::io::AsyncBufReadExt;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{info, warn};

use super::stream::StreamCtx;

// ---------------------------------------------------------------------------
// ndjson record produced in --kubernetes mode
// ---------------------------------------------------------------------------

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

    let mut lines = byte_stream.compat().lines();

    // Deduplicate: K8s can deliver the same line twice near stream start when
    // tailLines + follow=true are combined (known K8s quirk). Skip any line
    // whose timestamp is not strictly greater than the last forwarded one.
    let mut last_ts = String::new();

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                // K8s timestamp-prefixed lines: "2026-03-18T11:23:45.123Z <message>"
                let (ts, msg) = match line.find(' ') {
                    Some(i) => (&line[..i], &line[i + 1..]),
                    None => (line.as_str(), ""),
                };

                if ts <= last_ts.as_str() {
                    continue; // duplicate or out-of-order — skip
                }

                if !filter.allow(msg) {
                    continue;
                }

                let entry = LogEntry {
                    timestamp: ts,
                    namespace: &namespace,
                    pod: &pod,
                    container: container_str,
                    stream: stream_name,
                    message: msg,
                };
                if let Ok(mut json) = serde_json::to_string(&entry) {
                    json.push('\n');
                    if tx.try_send(json).is_err() {
                        warn!(pod = %pod, stream = stream_name, "send buffer full — dropping log line");
                    }
                }
                last_ts = ts.to_owned();
            }
            Ok(None) => {
                info!(pod = %pod, stream = stream_name, "log stream EOF");
                return StreamEnd::Closed;
            }
            Err(e) => {
                warn!(pod = %pod, stream = stream_name, error = %e, "log stream error");
                return StreamEnd::Closed;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_error(code: u16, message: &str) -> kube::Error {
        kube::Error::Api(kube::core::ErrorResponse {
            status: "Failure".into(),
            message: message.into(),
            reason: "Invalid".into(),
            code,
        })
    }

    #[test]
    fn rejected_stream_param_is_recognised() {
        // The answer of a 1.35 API server without the feature gate.
        let e = api_error(
            422,
            r#"PodLogOptions "logger-a" is invalid: stream: Forbidden: may not be specified"#,
        );
        assert!(is_stream_param_rejected(&e));
        // Other errors keep the separate streams.
        assert!(!is_stream_param_rejected(&api_error(
            422,
            "tailLines: Invalid value"
        )));
        assert!(!is_stream_param_rejected(&api_error(
            403,
            "stream: Forbidden"
        )));
    }
}
