use tokio::sync::mpsc;
use vigil_types::lines::Line;

use super::*;
use crate::LineFilter;

fn line(text: &str, continued: bool) -> Line {
    Line {
        text: text.to_string(),
        continued,
    }
}

fn accept(d: &mut Dedup, text: &str, continued: bool) -> Option<(String, String)> {
    d.accept(line(text, continued))
        .map(|(ts, msg)| (ts, msg.text))
}

fn pair(ts: &str, msg: &str) -> Option<(String, String)> {
    Some((ts.to_string(), msg.to_string()))
}

const T1: &str = "2026-11-09T01:52:46.1Z";
const T2: &str = "2026-11-09T01:52:46.2Z";
const T3: &str = "2026-11-09T01:52:47Z";

#[test]
fn dedup_skips_old_timestamps() {
    let mut d = Dedup::default();
    assert_eq!(accept(&mut d, &format!("{T2} b"), false), pair(T2, "b"));
    assert_eq!(accept(&mut d, &format!("{T1} a"), false), None, "older");
    assert_eq!(
        accept(&mut d, &format!("{T2} b"), false),
        None,
        "the same again"
    );
    assert_eq!(accept(&mut d, T3, false), pair(T3, ""));
}

#[test]
fn dedup_compares_points_in_time_not_text() {
    // The kubelet cuts trailing zeros: .12345 (= .123450000) is before
    // .123456789, although "5Z" sorts after "56" as text.
    let mut d = Dedup::default();
    let a = "2026-11-09T01:52:46.12345Z";
    let b = "2026-11-09T01:52:46.123456789Z";
    assert_eq!(
        accept(&mut d, &format!("{a} first"), false),
        pair(a, "first")
    );
    assert_eq!(
        accept(&mut d, &format!("{b} second"), false),
        pair(b, "second")
    );
    // Whole seconds have no fraction at all.
    let c = "2026-11-09T01:52:47Z";
    assert_eq!(
        accept(&mut d, &format!("{c} third"), false),
        pair(c, "third")
    );
}

#[test]
fn a_line_without_a_readable_timestamp_is_forwarded() {
    let mut d = Dedup::default();
    assert_eq!(accept(&mut d, &format!("{T2} b"), false), pair(T2, "b"));
    assert_eq!(
        accept(&mut d, "garbage text", false),
        pair("garbage", "text")
    );
    // The last readable timestamp still counts.
    assert_eq!(accept(&mut d, &format!("{T1} a"), false), None);
}

#[test]
fn parts_of_a_split_line_follow_its_first_part() {
    let mut d = Dedup::default();
    assert_eq!(
        accept(&mut d, &format!("{T2} first"), false),
        pair(T2, "first")
    );
    assert_eq!(
        accept(&mut d, "rest with spaces", true),
        pair(T2, "rest with spaces")
    );
    // A skipped line's parts are skipped too.
    assert_eq!(accept(&mut d, &format!("{T1} old"), false), None);
    assert_eq!(accept(&mut d, "old rest", true), None);
}

#[tokio::test]
async fn records_filter_whole_entries_and_parts_follow_the_first() {
    let filter = LineFilter::from_strs(&[], &["drop"]);
    let (tx, mut rx) = mpsc::channel(8);
    let mut r = Records {
        namespace: "ns",
        pod: "p",
        container: None,
        stream: "stdout",
        filter: &filter,
        tx: &tx,
        allow_parts: false,
    };
    r.send("T1", &line("keep\n\tat x", false));
    r.send("T1", &line("part with drop in it", true));
    r.send("T2", &line("drop me", false));
    r.send("T2", &line("harmless part", true));
    drop(tx);
    let mut got = Vec::new();
    while let Some(json) = rx.recv().await {
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        got.push(v["message"].as_str().unwrap().to_string());
    }
    assert_eq!(got, ["keep\n\tat x", "part with drop in it"]);
}
