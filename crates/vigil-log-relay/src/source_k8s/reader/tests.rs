use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use vigil_types::lines::Rule;
use vigil_types::plan::MultilinePreset;

use super::*;
use crate::LineFilter;

/// The messages of the records that `input` gives, read with `config` and
/// `filter`.
async fn messages(input: &str, config: JoinConfig, filter: LineFilter) -> Vec<(String, String)> {
    let (tx, mut rx) = mpsc::channel(64);
    let mut records = Records {
        namespace: "ns",
        pod: "p",
        container: None,
        stream: "output",
        filter: &filter,
        tx: &tx,
        allow_parts: false,
    };
    forward_lines(input.as_bytes(), &config, &mut records)
        .await
        .unwrap();
    drop(tx);
    let mut out = Vec::new();
    while let Some(json) = rx.recv().await {
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        out.push((
            v["timestamp"].as_str().unwrap().to_string(),
            v["message"].as_str().unwrap().to_string(),
        ));
    }
    out
}

fn java(max: usize) -> JoinConfig {
    JoinConfig {
        max,
        multiline: Some((Rule::Preset(MultilinePreset::Java), Duration::from_secs(10))),
    }
}

fn all() -> LineFilter {
    LineFilter::from_strs(&[], &[])
}

fn pair(ts: &str, msg: &str) -> (String, String) {
    (ts.to_string(), msg.to_string())
}

/// kubelet timestamps, as RFC3339Nano writes them: trailing zeros of the
/// fraction are cut.
const LOG: &str = "\
2026-11-09T02:04:43.58895Z ERROR request 1 failed
2026-11-09T02:04:43.588953242Z java.lang.IllegalStateException: boom
2026-11-09T02:04:43.5889533Z \tat com.example.A.b(A.java:1)
2026-11-09T02:04:43.588953401Z next
";

#[tokio::test]
async fn pod_lines_are_joined_with_the_first_timestamp_and_none_is_lost() {
    let got = messages(LOG, java(1024), all()).await;
    assert_eq!(
        got,
        [
            pair(
                "2026-11-09T02:04:43.58895Z",
                "ERROR request 1 failed\njava.lang.IllegalStateException: boom\n\tat com.example.A.b(A.java:1)"
            ),
            // Pending at the end of the stream, sent then.
            pair("2026-11-09T02:04:43.588953401Z", "next"),
        ]
    );
}

#[tokio::test]
async fn without_multiline_every_line_is_a_record() {
    let config = JoinConfig {
        max: 1024,
        multiline: None,
    };
    // The first line again, as K8s may send it near the stream start:
    // skipped.
    let log = format!("{LOG}2026-11-09T02:04:43.58895Z ERROR request 1 failed\n");
    let got = messages(&log, config, all()).await;
    assert_eq!(got.len(), 4, "{got:?}");
    assert_eq!(got[2].1, "\tat com.example.A.b(A.java:1)");
}

#[tokio::test]
async fn start_and_continue_regexes_join_pod_lines() {
    let log = "\
2026-11-09T02:04:43.1Z [a] one
2026-11-09T02:04:43.2Z more
2026-11-09T02:04:43.3Z [b] two
";
    let start = JoinConfig {
        max: 1024,
        multiline: Some((
            Rule::Start(regex::Regex::new(r"^\[").unwrap()),
            Duration::from_secs(10),
        )),
    };
    assert_eq!(
        messages(log, start, all()).await,
        [
            pair("2026-11-09T02:04:43.1Z", "[a] one\nmore"),
            pair("2026-11-09T02:04:43.3Z", "[b] two")
        ]
    );
    let cont = JoinConfig {
        max: 1024,
        multiline: Some((
            Rule::Continue(regex::Regex::new("^more").unwrap()),
            Duration::from_secs(10),
        )),
    };
    assert_eq!(
        messages(log, cont, all()).await,
        [
            pair("2026-11-09T02:04:43.1Z", "[a] one\nmore"),
            pair("2026-11-09T02:04:43.3Z", "[b] two")
        ]
    );
}

#[tokio::test]
async fn the_filter_decides_on_the_joined_record() {
    // "boom" is only in the second line of the trace.
    let filter = LineFilter::from_strs(&["boom"], &[]);
    let got = messages(LOG, java(1024), filter).await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].1.starts_with("ERROR request 1 failed\n"));
}

#[tokio::test]
async fn a_long_pod_line_comes_in_parts_with_its_timestamp() {
    let log = format!("2026-11-09T02:04:43.1Z {}\n", "x".repeat(2500));
    let config = JoinConfig {
        max: 1024,
        multiline: None,
    };
    let got = messages(&log, config, all()).await;
    let lens: Vec<usize> = got.iter().map(|(_, m)| m.len()).collect();
    // The line limit leaves room for the timestamp: the first part holds
    // max + TIMESTAMP_ROOM bytes of the raw line.
    assert_eq!(lens.iter().sum::<usize>(), 2500, "{lens:?}");
    assert!(lens.len() >= 3, "{lens:?}");
    assert!(got.iter().all(|(ts, _)| ts == "2026-11-09T02:04:43.1Z"));
}

#[tokio::test]
async fn a_pending_record_is_sent_after_the_timeout_while_the_stream_stays_open() {
    let (mut w, r) = tokio::io::duplex(1024);
    let config = JoinConfig {
        max: 1024,
        multiline: Some((
            Rule::Preset(MultilinePreset::Java),
            Duration::from_millis(30),
        )),
    };
    let filter = all();
    let (tx, mut rx) = mpsc::channel(8);
    let task = tokio::spawn(async move {
        let mut records = Records {
            namespace: "ns",
            pod: "p",
            container: None,
            stream: "output",
            filter: &filter,
            tx: &tx,
            allow_parts: false,
        };
        forward_lines(r, &config, &mut records).await
    });
    w.write_all(
        b"2026-11-09T02:04:43.1Z ERROR failed\n2026-11-09T02:04:43.2Z \tat a.B.c(B.java:1)\n",
    )
    .await
    .unwrap();
    let json = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("the timeout must complete the record")
        .unwrap();
    assert!(
        json.contains(r#""message":"ERROR failed\n\tat a.B.c(B.java:1)""#),
        "{json}"
    );
    drop(w);
    task.await.unwrap().unwrap();
}

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
