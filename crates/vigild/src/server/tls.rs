// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The HTTPS listener: bind, serve with the client certificate passed on.

use anyhow::Context;
use axum::Router;
use axum::body::Body;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
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

/// Marker injected into every request that arrives over the TLS listener,
/// with or without a client certificate. Lets the auth layer tell TLS
/// callers apart from Unix-socket callers.
#[derive(Clone, Copy, Debug)]
pub struct TlsConnection;

// ---------------------------------------------------------------------------
// HTTPS server
// ---------------------------------------------------------------------------

/// Bind the TLS API address (`--tls-addr`) before any service starts, so a
/// bind error is fatal at startup. Port `0` lets the kernel pick a free
/// port; the log line and `/v1/system-info` (`https-address`) show the one
/// actually bound.
pub fn bind_tls(addr: &str) -> anyhow::Result<TcpListener> {
    let listener =
        std::net::TcpListener::bind(addr).with_context(|| format!("binding TLS address {addr}"))?;
    listener.set_nonblocking(true)?;
    let listener = TcpListener::from_std(listener)?;
    info!("TLS API listening on {}", listener.local_addr()?);
    Ok(listener)
}

/// Host part of a `--tls-addr` (for the self-signed certificate):
/// `127.0.0.1:8443` → `127.0.0.1`, `[::1]:8443` → `::1`, `myhost` →
/// `myhost`.
pub fn tls_hostname(addr: &str) -> &str {
    if let Some(rest) = addr.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match addr.rsplit_once(':') {
        // More colons: a bare IPv6 address without a port.
        Some((host, _)) if !host.contains(':') => host,
        _ => addr,
    }
}

/// Serve the API over TLS on an already bound listener (see [`bind_tls`]).
pub async fn serve_tls(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    router: Router,
) -> anyhow::Result<()> {
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
                                req.extensions_mut().insert(TlsConnection);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_hostname_strips_port_and_brackets() {
        assert_eq!(tls_hostname("127.0.0.1:8443"), "127.0.0.1");
        assert_eq!(tls_hostname("vigil.example.com:8443"), "vigil.example.com");
        assert_eq!(tls_hostname("[::1]:8443"), "::1");
        assert_eq!(tls_hostname("::1"), "::1");
        assert_eq!(tls_hostname("localhost"), "localhost");
    }
}
