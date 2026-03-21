// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

/// Shared process-launching helpers used by both service.rs and check.rs.
mod identity;
mod parse;
#[cfg(test)]
mod tests;

pub use identity::{resolve_gid, resolve_uid};
pub use parse::parse_command;
