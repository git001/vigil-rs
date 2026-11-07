// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The API servers: the Unix control socket and the optional HTTPS
//! listener.

mod tls;
mod unix;

pub use tls::{TlsConnection, TlsPeerCert, bind_tls, serve_tls, tls_hostname};
pub use unix::{DEFAULT_SOCKET_MODE, UnixPeerInfo, bind_unix, serve_unix};
