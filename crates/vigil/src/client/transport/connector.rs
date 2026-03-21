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
pub struct UnixConnector(pub Arc<PathBuf>);

impl Service<Uri> for UnixConnector {
    type Response = TokioIo<UnixStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let path = Arc::clone(&self.0);
        Box::pin(async move {
            let stream = UnixStream::connect(path.as_path()).await?;
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
    pub fn new_unix(path: PathBuf) -> Self {
        let connector = UnixConnector(Arc::new(path));
        Transport::Unix(Box::new(
            Client::builder(TokioExecutor::new()).build(connector),
        ))
    }
}
