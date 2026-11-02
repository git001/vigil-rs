// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

/// Shared process-launching helpers used by both service.rs and check.rs.
mod env;
mod identity;
mod parse;
#[cfg(test)]
mod tests;

pub use env::{EnvPolicy, apply_env, glob_match};
pub use identity::{Credentials, apply_credentials, resolve_credentials, resolve_gid, resolve_uid};
pub use parse::parse_command;
