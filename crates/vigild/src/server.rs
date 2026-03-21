// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use anyhow::Context;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::Connected;
use axum::serve::IncomingStream;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use std::path::Path;
use tokio::net::{TcpListener, UnixListener};
use tokio_rustls::TlsAcceptor;
use tower::Service;
use tracing::info;

// ---------------------------------------------------------------------------
// TLS peer certificate passed via request extension
// ---------------------------------------------------------------------------

/// DER-encoded end-entity certificate presented by the TLS client.
///
/// Injected into every request that arrives over the TLS listener when the
/// client presents a certificate during the handshake.  Absent when no client
/// cert was presented or on Unix-socket connections.
#[derive(Clone)]
pub struct TlsPeerCert(pub Vec<u8>);

// ---------------------------------------------------------------------------
// Unix peer credentials passed via ConnectInfo
// ---------------------------------------------------------------------------

/// Peer credentials extracted from an incoming Unix socket connection.
#[derive(Clone, Debug)]
pub struct UnixPeerInfo {
    /// Effective UID of the connecting process, if retrievable.
    pub uid: Option<u32>,
}

impl Connected<IncomingStream<'_, UnixListener>> for UnixPeerInfo {
    fn connect_info(target: IncomingStream<'_, UnixListener>) -> Self {
        // IncomingStream::io() returns &UnixStream for UnixListener
        let uid = target.io().peer_cred().ok().map(|c| c.uid());
        UnixPeerInfo { uid }
    }
}

// ---------------------------------------------------------------------------
// Unix socket server
// ---------------------------------------------------------------------------

pub async fn serve_unix(socket_path: &Path, router: Router) -> anyhow::Result<()> {
    if socket_path.exists() {
        std::fs::remove_file(socket_path)
            .with_context(|| format!("removing stale socket {socket_path:?}"))?;
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating socket dir {parent:?}"))?;
    }
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("binding Unix socket {socket_path:?}"))?;
    info!("API listening on {socket_path:?}");
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<UnixPeerInfo>(),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTPS server
// ---------------------------------------------------------------------------

pub async fn serve_tls(addr: &str, acceptor: TlsAcceptor, router: Router) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding TLS address {addr}"))?;
    info!("TLS API listening on {addr}");

    loop {
        let (stream, peer) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let router = router.clone();

        tokio::spawn(async move {
            match acceptor.accept(stream).await {
                Err(e) => tracing::warn!("TLS handshake from {peer}: {e}"),
                Ok(tls_stream) => {
                    // Extract the peer's end-entity certificate (if any) so
                    // that the auth layer can match it against TLS identities.
                    let peer_cert: Option<TlsPeerCert> = tls_stream
                        .get_ref()
                        .1
                        .peer_certificates()
                        .and_then(|certs| certs.first())
                        .map(|c| TlsPeerCert(c.to_vec()));

                    let io = TokioIo::new(tls_stream);
                    let svc =
                        hyper::service::service_fn(move |mut req: hyper::Request<Incoming>| {
                            let mut r = router.clone();
                            let cert = peer_cert.clone();
                            async move {
                                if let Some(c) = cert {
                                    req.extensions_mut().insert(c);
                                }
                                r.call(req.map(Body::new)).await
                            }
                        });
                    if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
                        tracing::debug!("HTTP/1 error from {peer}: {e}");
                    }
                }
            }
        });
    }
}
