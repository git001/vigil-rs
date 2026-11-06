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
#[cfg(test)]
pub use cert::cert_to_pem;
pub use cert::generate_self_signed;

// ---------------------------------------------------------------------------
// TlsAcceptor construction
// ---------------------------------------------------------------------------
pub use acceptor::{acceptor_from_der, acceptor_from_der_mtls, load_or_generate};

// ---------------------------------------------------------------------------
// Proxy-aware reqwest client builder
// ---------------------------------------------------------------------------
pub use proxy::build_http_client;
