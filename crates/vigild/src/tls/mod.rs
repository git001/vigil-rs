// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

mod acceptor;
mod cert;
mod proxy;
#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Certificate generation / PEM helpers
// ---------------------------------------------------------------------------
pub use cert::{cert_to_pem, generate_self_signed, load_pem_chain};

// ---------------------------------------------------------------------------
// TlsAcceptor construction
// ---------------------------------------------------------------------------
pub use acceptor::{acceptor_from_der, acceptor_from_der_mtls, load_or_generate};

// ---------------------------------------------------------------------------
// Proxy-aware reqwest client builder
// ---------------------------------------------------------------------------
pub use proxy::{HttpClientConfig, build_http_client, no_proxy_matches, parse_no_proxy};
