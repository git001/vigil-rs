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

/// Default permission bits of the control socket: owner only.
///
/// With no identities configured every caller is admin, so the file mode is
/// the only access control. Deployments that grant other UIDs access via
/// local identities widen it with `--socket-mode`.
pub const DEFAULT_SOCKET_MODE: u32 = 0o600;

/// Bind the control socket. Errors are meant to be fatal: a daemon that keeps
/// running without its socket would leave whatever node holds the path as the
/// endpoint clients talk to.
///
/// - A symlink at the path is refused (could redirect the bind elsewhere).
/// - Anything other than a socket is refused (never delete a regular file).
/// - A stale socket that cannot be removed (e.g. another user's socket in a
///   sticky directory) is an error.
/// - The socket is bound under a temporary name, chmod'ed, then renamed into
///   place, so it never exists at the final path with looser permissions.
///   The process-wide umask is not touched because services may be spawned
///   concurrently and would inherit it.
pub fn bind_unix(socket_path: &Path, mode: u32) -> anyhow::Result<UnixListener> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    match std::fs::symlink_metadata(socket_path) {
        Ok(md) => {
            let ft = md.file_type();
            anyhow::ensure!(
                !ft.is_symlink(),
                "refusing to use control socket path {socket_path:?}: it is a symlink"
            );
            anyhow::ensure!(
                ft.is_socket(),
                "refusing to replace {socket_path:?}: it exists and is not a socket"
            );
            std::fs::remove_file(socket_path).with_context(|| {
                format!("removing stale socket {socket_path:?} (owned by another user?)")
            })?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| format!("inspecting socket path {socket_path:?}"));
        }
    }

    let parent = match socket_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent).with_context(|| format!("creating socket dir {parent:?}"))?;

    // sun_path holds 108 bytes including the trailing NUL.
    const SUN_PATH_MAX: usize = 107;
    let path_len = |p: &Path| p.as_os_str().len();
    anyhow::ensure!(
        path_len(socket_path) <= SUN_PATH_MAX,
        "socket path {socket_path:?} is {} bytes, the limit for Unix sockets is {SUN_PATH_MAX}",
        path_len(socket_path)
    );

    // Short temporary name (".<pid hex>.tmp", at most 11 bytes) so that it
    // fits wherever the final path fits in practice.
    let tmp = parent.join(format!(".{:x}.tmp", std::process::id()));

    let std_listener = if path_len(&tmp) <= SUN_PATH_MAX {
        // Leftover from a crashed start with the same PID (PID 1 in containers).
        if let Ok(md) = std::fs::symlink_metadata(&tmp)
            && md.file_type().is_socket()
        {
            let _ = std::fs::remove_file(&tmp);
        }
        let l = std::os::unix::net::UnixListener::bind(&tmp)
            .with_context(|| format!("binding Unix socket {tmp:?}"))?;
        let finish = || -> anyhow::Result<()> {
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
                .with_context(|| format!("chmod {mode:o} {tmp:?}"))?;
            std::fs::rename(&tmp, socket_path)
                .with_context(|| format!("moving socket into place at {socket_path:?}"))?;
            Ok(())
        };
        if let Err(e) = finish() {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        l
    } else {
        // Only a socket name shorter than the temp name can get here: bind in
        // place and chmod right away (short window with umask permissions).
        let l = std::os::unix::net::UnixListener::bind(socket_path)
            .with_context(|| format!("binding Unix socket {socket_path:?}"))?;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("chmod {mode:o} {socket_path:?}"))?;
        l
    };

    std_listener.set_nonblocking(true)?;
    let listener = UnixListener::from_std(std_listener)?;
    info!("API listening on {socket_path:?} (mode {mode:o})");
    Ok(listener)
}

/// Serve the API on an already bound control socket (see [`bind_unix`]).
pub async fn serve_unix(listener: UnixListener, router: Router) -> anyhow::Result<()> {
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

#[cfg(test)]
mod bind_tests {
    use super::*;
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    fn mode_of(p: &Path) -> u32 {
        std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn binds_with_default_mode_and_no_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("sub/vigild.sock");
        let _l = bind_unix(&sock, DEFAULT_SOCKET_MODE).unwrap();
        assert!(
            std::fs::symlink_metadata(&sock)
                .unwrap()
                .file_type()
                .is_socket()
        );
        assert_eq!(mode_of(&sock), 0o600);
        let names: Vec<_> = std::fs::read_dir(sock.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("vigild.sock")],
            "temp socket left behind"
        );
        tokio::net::UnixStream::connect(&sock)
            .await
            .expect("connectable");
    }

    /// Regression: the temporary name used to be ".<name>.<pid>.tmp", so a
    /// final path close to the 107-byte limit failed to bind although it fits.
    #[tokio::test]
    async fn final_path_at_limit_binds() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().as_os_str().len() + 1;
        let name = "s".repeat(107 - base);
        let sock = dir.path().join(&name);
        assert_eq!(sock.as_os_str().len(), 107);
        let _l = bind_unix(&sock, DEFAULT_SOCKET_MODE).unwrap();
        assert_eq!(mode_of(&sock), 0o600);
        tokio::net::UnixStream::connect(&sock)
            .await
            .expect("connectable");
    }

    #[tokio::test]
    async fn too_long_path_has_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("x".repeat(120));
        let err = bind_unix(&sock, DEFAULT_SOCKET_MODE)
            .unwrap_err()
            .to_string();
        assert!(err.contains("limit for Unix sockets"), "{err}");
    }

    #[tokio::test]
    async fn custom_mode_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("vigild.sock");
        let _l = bind_unix(&sock, 0o660).unwrap();
        assert_eq!(mode_of(&sock), 0o660);
    }

    #[tokio::test]
    async fn replaces_stale_socket() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("vigild.sock");
        drop(std::os::unix::net::UnixListener::bind(&sock).unwrap());
        assert!(sock.exists() || std::fs::symlink_metadata(&sock).is_ok());
        let _l = bind_unix(&sock, DEFAULT_SOCKET_MODE).unwrap();
        tokio::net::UnixStream::connect(&sock)
            .await
            .expect("new socket live");
    }

    #[tokio::test]
    async fn refuses_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere.sock");
        let sock = dir.path().join("vigild.sock");
        std::os::unix::fs::symlink(&target, &sock).unwrap();
        let err = bind_unix(&sock, DEFAULT_SOCKET_MODE).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(
            std::fs::symlink_metadata(&target).is_err(),
            "nothing created at target"
        );
    }

    #[tokio::test]
    async fn refuses_to_delete_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("vigild.sock");
        std::fs::write(&sock, b"important").unwrap();
        let err = bind_unix(&sock, DEFAULT_SOCKET_MODE).unwrap_err();
        assert!(err.to_string().contains("not a socket"), "{err}");
        assert_eq!(std::fs::read(&sock).unwrap(), b"important");
    }

    #[tokio::test]
    async fn unremovable_stale_socket_is_an_error() {
        if nix::unistd::Uid::effective().is_root() {
            return; // root can remove anything
        }
        let dir = tempfile::tempdir().unwrap();
        let held = dir.path().join("held");
        std::fs::create_dir(&held).unwrap();
        let sock = held.join("vigild.sock");
        drop(std::os::unix::net::UnixListener::bind(&sock).unwrap());
        std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o555)).unwrap();
        let res = bind_unix(&sock, DEFAULT_SOCKET_MODE);
        std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = res.unwrap_err();
        assert!(err.to_string().contains("removing stale socket"), "{err}");
    }
}
