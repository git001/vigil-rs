use std::time::Duration;

use tokio::time::Instant;
use vigil_types::lines::Rule;
use vigil_types::plan::MultilinePreset;

use super::*;

fn java() -> Keyed {
    Keyed::new(JoinConfig {
        max: 1024,
        multiline: Some((Rule::Preset(MultilinePreset::Java), Duration::from_secs(10))),
    })
}

/// A record as vigild writes it.
fn rec(service: &str, message: &str) -> String {
    let json = |s: &str| serde_json::to_string(s).unwrap();
    format!(
        r#"{{"timestamp":{},"service":"{service}","stream":"stdout","message":{}}}"#,
        json(&format!("T-{message}")),
        json(message)
    )
}

fn push(k: &mut Keyed, payload: &str) -> Option<String> {
    k.push(payload.to_string(), false, Instant::now())
        .map(|(p, _)| p)
}

#[test]
fn records_are_joined_per_service_on_their_message() {
    let mut k = java();
    assert_eq!(push(&mut k, &rec("api", "ERROR failed")), None);
    // Another service in between does not break the trace of the first.
    assert_eq!(push(&mut k, &rec("web", "GET /")), None);
    assert_eq!(
        push(&mut k, &rec("api", "java.lang.IllegalStateException: boom")),
        None
    );
    assert_eq!(push(&mut k, &rec("api", "\tat a.B.c(B.java:1)")), None);
    assert_eq!(
        push(&mut k, &rec("web", "GET /x")),
        Some(rec("web", "GET /"))
    );
    // The joined record keeps the fields of its first record, in order.
    let joined = push(&mut k, &rec("api", "next")).unwrap();
    assert_eq!(
        joined,
        r#"{"timestamp":"T-ERROR failed","service":"api","stream":"stdout","message":"ERROR failed\njava.lang.IllegalStateException: boom\n\tat a.B.c(B.java:1)"}"#
    );
    let mut rest: Vec<String> = k.flush(None).into_iter().map(|(p, _)| p).collect();
    rest.sort();
    let mut want = [rec("api", "next"), rec("web", "GET /x")];
    want.sort();
    assert_eq!(rest, want);
    assert_eq!(k.deadline(), None);
}

#[test]
fn lines_that_are_no_records_are_joined_as_text() {
    let mut k = java();
    assert_eq!(push(&mut k, "ERROR failed"), None);
    assert_eq!(push(&mut k, "\tat a.B.c(B.java:1)"), None);
    assert_eq!(
        push(&mut k, "{\"no\":\"message\"}"),
        Some("ERROR failed\n\tat a.B.c(B.java:1)".into())
    );
}

#[test]
fn only_due_entries_are_flushed() {
    let mut k = java();
    let t0 = Instant::now();
    k.push(rec("api", "a"), false, t0);
    k.push(rec("web", "b"), false, t0 + Duration::from_secs(5));
    assert_eq!(k.deadline(), Some(t0 + Duration::from_secs(10)));
    let due = k.flush(Some(t0 + Duration::from_secs(10)));
    assert_eq!(due, [(rec("api", "a"), false)]);
    assert_eq!(k.deadline(), Some(t0 + Duration::from_secs(15)));
}

#[test]
fn without_a_rule_payloads_pass_unchanged() {
    let mut k = Keyed::new(JoinConfig {
        max: 1024,
        multiline: None,
    });
    let raw = r#"{"message": "spaced",  "service":"x"}"#;
    assert_eq!(push(&mut k, raw), Some(raw.to_string()));
    assert_eq!(k.deadline(), None);
}
