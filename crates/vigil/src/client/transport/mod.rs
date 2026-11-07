// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Low-level transport: the Unix-socket connector, sending a request over
//! the Unix socket or HTTP(S), and reading what vigild sends back.

mod connector;
mod encoding;
mod send;
#[cfg(test)]
mod tests;

pub(super) use connector::Transport;
pub(super) use encoding::{drain_sse_buf, names_query, parse, parse_void};
#[cfg(test)]
use send::unix_uri;
