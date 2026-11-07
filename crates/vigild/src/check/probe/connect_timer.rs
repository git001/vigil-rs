// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The client of an HTTP check, which also measures how long its
//! connections take to come up (`vigil_check_connect_duration_seconds`).
//!
//! A tower layer around reqwest's connector times each connect: DNS, TCP,
//! the TLS handshake and a proxy tunnel, until the connection is ready for
//! the request. The client keeps no idle connections, so every probe
//! connects and is measured.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tower::{Layer, Service};

/// The last connect time of a client's connector.
type Slot = Arc<Mutex<Option<Duration>>>;

/// An HTTP check's client and the time its last connection took.
#[derive(Clone, Debug)]
pub struct CheckClient {
    pub(crate) http: reqwest::Client,
    connect: Slot,
}

impl CheckClient {
    /// Build the client from `builder`, which holds the check's settings
    /// (redirects, `tls:`, `proxy:`).
    pub(crate) fn build(builder: reqwest::ClientBuilder) -> reqwest::Result<Self> {
        let connect = Slot::default();
        let http = builder
            .pool_max_idle_per_host(0)
            .connector_layer(ConnectTimerLayer(Arc::clone(&connect)))
            .build()?;
        Ok(Self { http, connect })
    }

    /// Forget the last measurement (before a probe).
    pub(crate) fn reset_connect(&self) {
        *self.connect.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// How long the last connection took to come up; `None` if no
    /// connection came up since `reset_connect`.
    pub(crate) fn connect_time(&self) -> Option<Duration> {
        *self.connect.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Clone)]
struct ConnectTimerLayer(Slot);

impl<S> Layer<S> for ConnectTimerLayer {
    type Service = ConnectTimer<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ConnectTimer {
            inner,
            slot: Arc::clone(&self.0),
        }
    }
}

#[derive(Clone)]
struct ConnectTimer<S> {
    inner: S,
    slot: Slot,
}

impl<S, R> Service<R> for ConnectTimer<S>
where
    S: Service<R>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: R) -> Self::Future {
        let started = Instant::now();
        let connecting = self.inner.call(request);
        let slot = Arc::clone(&self.slot);
        Box::pin(async move {
            let conn = connecting.await;
            if conn.is_ok() {
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(started.elapsed());
            }
            conn
        })
    }
}
