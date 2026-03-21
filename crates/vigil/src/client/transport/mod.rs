// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Low-level transport layer: Unix-socket connector and HTTP/Unix send helpers.

mod connector;
mod http;
mod unix;
#[cfg(test)]
mod tests;

pub(super) use connector::Transport;
pub(super) use http::{drain_sse_buf, http_parse, http_parse_void, names_query};
pub(super) use unix::{unix_send, unix_send_void, unix_uri};
