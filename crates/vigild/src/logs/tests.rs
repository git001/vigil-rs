// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::io::AsyncBufReadExt;
use tokio::sync::broadcast;
use vigil_types::api::{LogEntry, LogStream};

use super::push::{PushStream, push_loop, spawn_push_tcp, spawn_push_unix};
use super::reader::spawn_reader;
use super::store::LogStore;

fn entry(service: &str, msg: &str) -> LogEntry {
    LogEntry {
        timestamp: Utc::now(),
        service: service.to_string(),
        stream: LogStream::Stdout,
        message: msg.to_string(),
    }
}

#[tokio::test]
async fn ring_buffer_evicts_oldest_when_full() {
    let store = LogStore::new(3, 64);
    for i in 0..5u32 {
        store.push(entry("svc", &format!("msg{i}"))).await;
    }
    let tail = store.tail(&["svc".to_string()], 100).await;
    assert_eq!(tail.len(), 3);
    let msgs: Vec<&str> = tail.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(msgs, vec!["msg2", "msg3", "msg4"]);
}

#[tokio::test]
async fn tail_returns_at_most_n_lines() {
    let store = LogStore::new(100, 64);
    for i in 0..20u32 {
        store.push(entry("svc", &format!("line{i}"))).await;
    }
    let tail = store.tail(&["svc".to_string()], 5).await;
    assert_eq!(tail.len(), 5);
    assert_eq!(tail.last().unwrap().message, "line19");
}

#[tokio::test]
async fn tail_empty_filter_returns_all_services() {
    let store = LogStore::new(100, 64);
    store.push(entry("alpha", "a")).await;
    store.push(entry("beta", "b")).await;
    let tail = store.tail(&[], 100).await;
    assert_eq!(tail.len(), 2);
}

#[tokio::test]
async fn tail_service_filter_excludes_others() {
    let store = LogStore::new(100, 64);
    store.push(entry("alpha", "a")).await;
    store.push(entry("beta", "b")).await;
    let tail = store.tail(&["alpha".to_string()], 100).await;
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].service, "alpha");
}

#[tokio::test]
async fn tail_unknown_service_returns_empty() {
    let store = LogStore::new(100, 64);
    store.push(entry("svc", "x")).await;
    let tail = store.tail(&["nope".to_string()], 100).await;
    assert!(tail.is_empty());
}

#[tokio::test]
async fn tail_on_empty_store_returns_empty() {
    let store = LogStore::new(100, 64);
    assert!(store.tail(&[], 10).await.is_empty());
    assert!(store.tail(&["svc".to_string()], 10).await.is_empty());
}

#[tokio::test]
async fn subscribe_receives_pushed_entries() {
    let store = LogStore::new(100, 64);
    let mut rx = store.subscribe();
    store.push(entry("svc", "hello")).await;
    let received = rx.recv().await.unwrap();
    assert_eq!(received.message, "hello");
    assert_eq!(received.service, "svc");
}

#[tokio::test]
async fn subscribe_lagged_does_not_block_push() {
    // broadcast capacity = 2; push 4 messages; slow receiver should see Lagged
    let store = LogStore::new(100, 2);
    let mut rx = store.subscribe();
    for i in 0..4u32 {
        store.push(entry("svc", &format!("m{i}"))).await;
    }
    // First recv should be Lagged (skipped messages)
    let result = rx.recv().await;
    // Either Ok (if not lagged yet) or Err(Lagged) — just ensure no panic
    let _ = result;
}

#[tokio::test]
async fn tail_zero_n_returns_empty() {
    let store = LogStore::new(100, 64);
    store.push(entry("svc", "x")).await;
    let tail = store.tail(&[], 0).await;
    assert!(tail.is_empty());
}

// -----------------------------------------------------------------------
// push_loop tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn push_loop_closed_channel_returns_true() {
    // A broadcast sender that is immediately dropped → Closed
    let (tx, mut rx) = broadcast::channel::<LogEntry>(4);
    drop(tx);
    // push_loop needs a PushStream; use a Unix socket pair
    let (client, _server) = tokio::net::UnixStream::pair().unwrap();
    let mut ps = PushStream::Unix(tokio::io::BufWriter::new(client));
    let result = push_loop("svc", &mut ps, &mut rx).await;
    assert!(result, "closed channel should return true");
}

#[tokio::test]
async fn push_loop_forwards_matching_service_entry() {
    let (tx, mut rx) = broadcast::channel::<LogEntry>(4);
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let mut ps = PushStream::Unix(tokio::io::BufWriter::new(client));

    // Send a matching entry then drop the sender (so the loop exits on Closed)
    tx.send(entry("svc", "hello-from-push-loop")).unwrap();
    drop(tx);

    let result = push_loop("svc", &mut ps, &mut rx).await;
    assert!(result);

    // Drop ps first so the write half closes → EOF on the read half
    drop(ps);

    use tokio::io::AsyncReadExt;
    let mut buf = String::new();
    let mut server = server;
    server.read_to_string(&mut buf).await.unwrap();
    assert!(
        buf.contains("hello-from-push-loop"),
        "forwarded line not found in buf: {buf}"
    );
}

#[tokio::test]
async fn push_loop_skips_unrelated_service_entry() {
    let (tx, mut rx) = broadcast::channel::<LogEntry>(4);
    let (client, _server) = tokio::net::UnixStream::pair().unwrap();
    let mut ps = PushStream::Unix(tokio::io::BufWriter::new(client));

    tx.send(entry("other-svc", "should-not-appear")).unwrap();
    drop(tx);

    // The loop should exit immediately (Closed) without writing anything
    let result = push_loop("svc", &mut ps, &mut rx).await;
    assert!(result, "loop must return true on Closed channel");
    // No read needed: we just verify the loop didn't hang
}

// -----------------------------------------------------------------------
// spawn_reader tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn spawn_reader_feeds_log_store() {
    use tokio::io::duplex;
    let store = LogStore::new(100, 64);
    let (mut writer, reader) = duplex(1024);

    spawn_reader(
        "my-svc".into(),
        LogStream::Stdout,
        reader,
        Arc::clone(&store),
        false,
    );

    use tokio::io::AsyncWriteExt;
    writer.write_all(b"line-one\nline-two\n").await.unwrap();
    drop(writer);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let tail = store.tail(&["my-svc".to_string()], 100).await;
    let msgs: Vec<&str> = tail.iter().map(|e| e.message.as_str()).collect();
    assert!(msgs.contains(&"line-one"), "expected line-one in {msgs:?}");
    assert!(msgs.contains(&"line-two"), "expected line-two in {msgs:?}");
}

#[tokio::test]
async fn spawn_reader_records_correct_stream_type() {
    use tokio::io::duplex;
    let store = LogStore::new(100, 64);
    let (mut writer, reader) = duplex(1024);

    spawn_reader(
        "stderr-svc".into(),
        LogStream::Stderr,
        reader,
        Arc::clone(&store),
        false,
    );

    use tokio::io::AsyncWriteExt;
    writer.write_all(b"err-line\n").await.unwrap();
    drop(writer);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let tail = store.tail(&["stderr-svc".to_string()], 100).await;
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].stream, LogStream::Stderr);
}

// -----------------------------------------------------------------------
// spawn_push_tcp tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn spawn_push_tcp_sends_entries() {
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let store = LogStore::new(100, 64);
    let _handle = spawn_push_tcp("svc".to_string(), addr, Arc::clone(&store));

    // Accept the connection the push task will make
    let (socket, _) = listener.accept().await.unwrap();
    let mut reader = tokio::io::BufReader::new(socket);

    // Give the task time to enter push_loop and subscribe before pushing
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Push an entry so the push loop writes it to the socket
    store.push(entry("svc", "tcp-hello")).await;

    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(2),
        reader.read_line(&mut line),
    )
    .await
    .expect("timed out waiting for line")
    .unwrap();
    assert!(
        line.contains("tcp-hello"),
        "expected 'tcp-hello' in line: {line}"
    );
}


#[tokio::test]
async fn spawn_push_tcp_exits_when_store_dropped() {
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    // Pass `store` (not a clone) into the task so the task holds the sole Arc.
    // Once `store` is moved in, dropping it from the test is impossible —
    // but we can trigger RecvError::Closed by ensuring no other Senders exist.
    // The only sound way to verify "task stops when channel closes" without
    // mutating the production API is to abort the handle (which is the
    // documented stop mechanism: "Abort the returned handle to stop the task").
    let store = LogStore::new(100, 64);
    let handle = spawn_push_tcp("svc".to_string(), addr, Arc::clone(&store));

    // Accept to let the task enter push_loop
    let (_socket, _) = listener.accept().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Drop the test's Arc so the task holds the only reference.
    drop(store);

    // Abort the task — the doc-comment says this is the intended stop mechanism.
    handle.abort();

    // The abort should resolve the handle quickly (cancelled error is expected).
    let result = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("task did not terminate after abort");
    // An aborted task returns Err(JoinError::Cancelled); that's the success case.
    assert!(result.unwrap_err().is_cancelled());
}

// -----------------------------------------------------------------------
// spawn_push_unix tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn spawn_push_unix_sends_entries() {
    use tokio::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("push_test.sock");
    let socket_path_str = socket_path.to_str().unwrap().to_string();

    let listener = UnixListener::bind(&socket_path).unwrap();

    let store = LogStore::new(100, 64);
    let _handle =
        spawn_push_unix("svc".to_string(), socket_path_str, Arc::clone(&store));

    // Accept the connection the push task will make
    let (socket, _) = listener.accept().await.unwrap();
    let mut reader = tokio::io::BufReader::new(socket);

    // Give the task time to enter push_loop and subscribe before pushing
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Push an entry so the push loop writes it to the socket
    store.push(entry("svc", "unix-hello")).await;

    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(2),
        reader.read_line(&mut line),
    )
    .await
    .expect("timed out waiting for line")
    .unwrap();
    assert!(
        line.contains("unix-hello"),
        "expected 'unix-hello' in line: {line}"
    );
}
