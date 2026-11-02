// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body_util::Full;
use hyper::Uri;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::UnixStream;
use tower::Service;

// ---------------------------------------------------------------------------
// Unix socket connector (hyper)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct UnixConnector {
    pub path: Arc<PathBuf>,
    /// Additional daemon UID to trust besides root and our own euid.
    pub trusted_uid: Option<u32>,
}

fn peer_allowed(peer: u32, me: u32, trusted_uid: Option<u32>) -> bool {
    peer == 0 || peer == me || Some(peer) == trusted_uid
}

/// Refuse to talk to a listener that is neither root, our own euid, nor the
/// explicitly trusted UID. Otherwise another local user who managed to place
/// a socket at the path would receive our commands (and credentials in the
/// future) and could feed us forged replies.
pub(crate) fn check_peer(
    stream: &UnixStream,
    path: &std::path::Path,
    trusted_uid: Option<u32>,
) -> std::io::Result<()> {
    let peer = stream.peer_cred()?.uid();
    let me = nix::unistd::geteuid().as_raw();
    if peer_allowed(peer, me, trusted_uid) {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!(
            "refusing to talk to {}: listener runs as uid {peer}, expected root or uid {me} \
             (use --socket-uid {peer} if this daemon is trusted)",
            path.display()
        ),
    ))
}

impl Service<Uri> for UnixConnector {
    type Response = TokioIo<UnixStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let path = Arc::clone(&self.path);
        let trusted_uid = self.trusted_uid;
        Box::pin(async move {
            let stream = UnixStream::connect(path.as_path()).await?;
            check_peer(&stream, &path, trusted_uid)?;
            Ok(TokioIo::new(stream))
        })
    }
}

// ---------------------------------------------------------------------------
// Transport enum
// ---------------------------------------------------------------------------

pub enum Transport {
    Unix(Box<Client<UnixConnector, Full<Bytes>>>),
    Http {
        client: reqwest::Client,
        base_url: String,
    },
}

impl Transport {
    pub fn new_unix(path: PathBuf, trusted_uid: Option<u32>) -> Self {
        let connector = UnixConnector {
            path: Arc::new(path),
            trusted_uid,
        };
        Transport::Unix(Box::new(
            Client::builder(TokioExecutor::new()).build(connector),
        ))
    }
}

#[cfg(test)]
mod peer_tests {
    use super::*;

    #[test]
    fn peer_allowed_rules() {
        assert!(peer_allowed(0, 1000, None), "root daemon");
        assert!(peer_allowed(1000, 1000, None), "own daemon");
        assert!(!peer_allowed(1001, 1000, None), "foreign daemon");
        assert!(peer_allowed(1001, 1000, Some(1001)), "explicitly trusted");
        assert!(!peer_allowed(1002, 1000, Some(1001)));
    }

    #[tokio::test]
    async fn own_listener_passes_check() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("s.sock");
        let _l = tokio::net::UnixListener::bind(&sock).unwrap();
        let stream = UnixStream::connect(&sock).await.unwrap();
        check_peer(&stream, &sock, None).unwrap();
    }
}
