// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use tokio::io::{AsyncWriteExt, BufReader};

use super::*;

/// All lines of `input`, read with `max`, as (text, continued) pairs.
async fn read_all(input: &[u8], max: usize) -> Vec<(String, bool)> {
    let mut r = LineReader::new(input, max);
    let mut out = Vec::new();
    while let Some(l) = r.next_line().await.unwrap() {
        out.push((l.text, l.continued));
    }
    out
}

fn whole(lines: &[&str]) -> Vec<(String, bool)> {
    lines.iter().map(|l| (l.to_string(), false)).collect()
}

#[tokio::test]
async fn splits_at_newlines_and_removes_a_carriage_return() {
    let got = read_all(b"a\nb\r\n\nlast", 100).await;
    assert_eq!(got, whole(&["a", "b", "", "last"]));
}

#[tokio::test]
async fn empty_input_has_no_lines() {
    assert!(read_all(b"", 100).await.is_empty());
}

#[tokio::test]
async fn invalid_utf8_becomes_the_replacement_character_and_reading_goes_on() {
    let got = read_all(b"before\n\xff\xfe bad\nafter\n", 100).await;
    assert_eq!(got, whole(&["before", "\u{fffd}\u{fffd} bad", "after"]));
}

#[tokio::test]
async fn a_line_of_exactly_max_bytes_is_not_split() {
    let got = read_all(b"abcd\nef\n", 4).await;
    assert_eq!(got, whole(&["abcd", "ef"]));
}

#[tokio::test]
async fn a_longer_line_comes_in_parts() {
    let got = read_all(b"abcdefghij\nk\n", 4).await;
    assert_eq!(
        got,
        vec![
            ("abcd".to_string(), false),
            ("efgh".to_string(), true),
            ("ij".to_string(), true),
            ("k".to_string(), false),
        ]
    );
}

#[tokio::test]
async fn a_split_never_cuts_a_character() {
    // "aé€" is a, then 2 bytes, then 3 bytes; with max 4 the euro sign
    // does not fit after "aé" and goes to the next part whole.
    let got = read_all("aé€x\n".as_bytes(), 4).await;
    assert_eq!(
        got,
        vec![("aé".to_string(), false), ("€x".to_string(), true)]
    );
}

#[tokio::test]
async fn a_line_without_newline_is_bounded_in_memory() {
    // 4 MiB without a newline, read in parts of 128 KiB.
    let input = vec![b'x'; 4 * 1024 * 1024];
    let got = read_all(&input, DEFAULT_MAX_LINE).await;
    assert_eq!(got.len(), 32);
    assert!(got.iter().all(|(t, _)| t.len() == DEFAULT_MAX_LINE));
    assert!(!got[0].1 && got[1..].iter().all(|(_, c)| *c));
}

#[tokio::test]
async fn lines_that_arrive_in_pieces_are_joined() {
    // A pipe that delivers a line in several reads, and a split character
    // across two of them.
    let (mut w, r) = tokio::io::duplex(64);
    let mut lines = LineReader::new(BufReader::new(r), 100);
    let writer = tokio::spawn(async move {
        for piece in [&b"hel"[..], b"lo \xe2\x82", b"\xac\n", b"next\n"] {
            w.write_all(piece).await.unwrap();
            w.flush().await.unwrap();
            tokio::task::yield_now().await;
        }
    });
    assert_eq!(lines.next_line().await.unwrap().unwrap().text, "hello €");
    assert_eq!(lines.next_line().await.unwrap().unwrap().text, "next");
    writer.await.unwrap();
    assert_eq!(lines.next_line().await.unwrap(), None);
}

#[test]
fn incomplete_tail_counts_the_bytes_of_a_cut_character() {
    assert_eq!(incomplete_tail(b"abc"), 0);
    assert_eq!(incomplete_tail("aé".as_bytes()), 0);
    assert_eq!(incomplete_tail(b"a\xc3"), 1);
    assert_eq!(incomplete_tail(b"a\xe2\x82"), 2);
    assert_eq!(incomplete_tail(b"a\xf0\x9f\x98"), 3);
    assert_eq!(incomplete_tail("a😀".as_bytes()), 0);
    // Stray continuation bytes are no start of a character.
    assert_eq!(incomplete_tail(b"\x80\x80\x80\x80"), 0);
}

// ---------------------------------------------------------------------------
// Multi-line entries
// ---------------------------------------------------------------------------

use std::time::Duration;

use crate::plan::{LogsMultiline, MultilinePreset};

const JAVA_TRACE: &str = "\
2026-11-09 10:15:30.123 ERROR 1 --- [nio-8080-exec-1] o.a.c.c.C.[dispatcherServlet] : Servlet.service() threw exception
java.lang.IllegalStateException: boom
\tat com.example.demo.DemoController.fail(DemoController.java:21)
\tat java.base/jdk.internal.reflect.DirectMethodHandleAccessor.invoke(DirectMethodHandleAccessor.java:103)
Caused by: java.io.IOException: disk full
\tat com.example.demo.Store.write(Store.java:40)
\t... 2 more
\tSuppressed: java.lang.RuntimeException: close failed
\t\tat com.example.demo.Store.close(Store.java:55)
2026-11-09 10:15:30.200  INFO 1 --- [nio-8080-exec-2] c.e.demo.DemoController : next request
";

const PYTHON_TRACE: &str = "\
2026-11-09 10:15:30,123 ERROR app: request failed
Traceback (most recent call last):
  File \"/app/app.py\", line 10, in handle
    load()
  File \"/app/app.py\", line 5, in load
    raise KeyError(\"id\")
KeyError: 'id'

The above exception was the direct cause of the following exception:

Traceback (most recent call last):
  File \"/app/app.py\", line 12, in handle
    raise RuntimeError(\"load failed\") from e
RuntimeError: load failed
2026-11-09 10:15:31,000 INFO app: next
";

fn preset(p: MultilinePreset) -> Rule {
    Rule::from_config(&LogsMultiline {
        preset: Some(p),
        ..Default::default()
    })
    .unwrap()
}

/// The entries of `input` with `rule`, `max` bytes, read to the end.
async fn entries(input: &str, rule: Rule, max: usize) -> Vec<String> {
    let lines = LineReader::new(input.as_bytes(), max);
    let m = Multiline::new(rule, max, Duration::from_secs(10));
    let mut r = EntryReader::new(lines, Some(m));
    let mut out = Vec::new();
    while let Some(e) = r.next_entry().await.unwrap() {
        out.push(e.text);
    }
    out
}

#[tokio::test]
async fn java_preset_keeps_a_stack_trace_with_its_log_line() {
    let got = entries(JAVA_TRACE, preset(MultilinePreset::Java), DEFAULT_MAX_LINE).await;
    let lines: Vec<&str> = JAVA_TRACE.lines().collect();
    assert_eq!(got, [lines[..9].join("\n"), lines[9].to_string()]);
}

#[tokio::test]
async fn python_preset_keeps_chained_tracebacks_with_their_log_line() {
    let got = entries(
        PYTHON_TRACE,
        preset(MultilinePreset::Python),
        DEFAULT_MAX_LINE,
    )
    .await;
    let lines: Vec<&str> = PYTHON_TRACE.lines().collect();
    assert_eq!(got, [lines[..14].join("\n"), lines[14].to_string()]);
}

#[tokio::test]
async fn indent_preset_joins_indented_lines_only() {
    let got = entries("a\n  b\n\tc\nd\n", preset(MultilinePreset::Indent), 100).await;
    assert_eq!(got, ["a\n  b\n\tc", "d"]);
}

#[tokio::test]
async fn start_and_continue_rules() {
    let start = Rule::Start(regex::Regex::new(r"^\d{4}-").unwrap());
    let got = entries("2026- a\nmore\n2026- b\n", start, 100).await;
    assert_eq!(got, ["2026- a\nmore", "2026- b"]);
    let cont = Rule::Continue(regex::Regex::new(r"^\+ ").unwrap());
    let got = entries("a\n+ b\nc\n", cont, 100).await;
    assert_eq!(got, ["a\n+ b", "c"]);
}

#[tokio::test]
async fn a_lone_first_line_and_unrelated_lines_stay_single() {
    let got = entries(
        "Error: no qualified class\nplain\n",
        preset(MultilinePreset::Java),
        100,
    )
    .await;
    assert_eq!(got, ["Error: no qualified class", "plain"]);
}

#[tokio::test]
async fn an_entry_never_grows_past_max() {
    // max 10: "a\n  bbbb" (8) fits, "\n  cc" would make it 13.
    let got = entries("a\n  bbbb\n  cc\n", preset(MultilinePreset::Indent), 10).await;
    assert_eq!(got, ["a\n  bbbb", "  cc"]);
}

#[tokio::test]
async fn the_parts_of_a_split_line_are_joined_up_to_max() {
    // max 8 for lines, so "abcdefghij" comes as "abcdefgh" + "ij"; the
    // entry limit is the same, so the parts stay apart.
    let got = entries("abcdefghij\n", preset(MultilinePreset::Indent), 8).await;
    assert_eq!(got, ["abcdefgh", "ij"]);
}

#[tokio::test]
async fn an_entry_is_complete_after_the_timeout() {
    let (mut w, r) = tokio::io::duplex(64);
    let lines = LineReader::new(BufReader::new(r), 100);
    let m = Multiline::new(
        preset(MultilinePreset::Indent),
        100,
        Duration::from_millis(30),
    );
    let mut entries = EntryReader::new(lines, Some(m));
    w.write_all(b"first\n  more\n").await.unwrap();
    // No further line: the entry comes after the timeout, while the
    // stream is still open.
    let got = tokio::time::timeout(Duration::from_secs(2), entries.next_entry())
        .await
        .expect("the timeout must complete the entry")
        .unwrap()
        .unwrap();
    assert_eq!(got.text, "first\n  more");
    w.write_all(b"second\n").await.unwrap();
    drop(w);
    assert_eq!(entries.next_entry().await.unwrap().unwrap().text, "second");
    assert_eq!(entries.next_entry().await.unwrap(), None);
}

#[tokio::test]
async fn without_a_rule_each_line_is_an_entry() {
    let lines = LineReader::new(&b"a\n  b\n"[..], 100);
    let mut r = EntryReader::new(lines, None);
    assert_eq!(r.next_entry().await.unwrap().unwrap().text, "a");
    assert_eq!(r.next_entry().await.unwrap().unwrap().text, "  b");
    assert_eq!(r.next_entry().await.unwrap(), None);
}

#[test]
fn rule_from_config_needs_exactly_one_valid_rule() {
    let err = |c: LogsMultiline| Rule::from_config(&c).unwrap_err();
    assert_eq!(
        err(LogsMultiline::default()),
        "logs-multiline: set exactly one of preset, start and continue"
    );
    assert_eq!(
        err(LogsMultiline {
            preset: Some(MultilinePreset::Java),
            start: Some("^x".into()),
            ..Default::default()
        }),
        "logs-multiline: set exactly one of preset, start and continue"
    );
    let bad = err(LogsMultiline {
        continue_: Some("(".into()),
        ..Default::default()
    });
    assert!(
        bad.starts_with("logs-multiline: continue: invalid regex: "),
        "{bad}"
    );
}

#[test]
fn multiline_from_config_checks_the_timeout() {
    let cfg = |t: &str| LogsMultiline {
        preset: Some(MultilinePreset::Java),
        timeout: Some(t.into()),
        ..Default::default()
    };
    assert!(Multiline::from_config(&cfg("10ms"), 100).is_ok());
    assert!(Multiline::from_config(&cfg("10s"), 100).is_ok());
    for (t, why) in [
        ("5ms", "must be between 10ms and 10s"),
        ("11s", "must be between 10ms and 10s"),
        ("soon", "is not a duration"),
    ] {
        let e = Multiline::from_config(&cfg(t), 100).err().unwrap();
        assert!(
            e.starts_with(&format!("logs-multiline: timeout \"{t}\" ")) && e.contains(why),
            "{e}"
        );
    }
}

/// Real output of a JVM and of CPython (see testdata/README.md): a log line,
/// a stack trace and one more log line each.
const REAL_TRACES: [(&str, &str, MultilinePreset); 7] = [
    (
        "java-deep200",
        include_str!("testdata/java-deep200.log"),
        MultilinePreset::Java,
    ),
    (
        "java-chained",
        include_str!("testdata/java-chained.log"),
        MultilinePreset::Java,
    ),
    (
        "java-stream",
        include_str!("testdata/java-stream.log"),
        MultilinePreset::Java,
    ),
    (
        "java-overflow",
        include_str!("testdata/java-overflow.log"),
        MultilinePreset::Java,
    ),
    (
        "python-chained",
        include_str!("testdata/python-chained.log"),
        MultilinePreset::Python,
    ),
    (
        "python-during",
        include_str!("testdata/python-during.log"),
        MultilinePreset::Python,
    ),
    (
        "python-group",
        include_str!("testdata/python-group.log"),
        MultilinePreset::Python,
    ),
];

#[tokio::test]
async fn presets_join_real_stack_traces_exactly() {
    for (name, log, p) in REAL_TRACES {
        let got = entries(log, preset(p), DEFAULT_MAX_LINE).await;
        let lines: Vec<&str> = log.lines().collect();
        let (last, trace) = lines.split_last().unwrap();
        assert_eq!(got, [trace.join("\n"), last.to_string()], "{name}");
    }
}
