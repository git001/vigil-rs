// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

// ---------------------------------------------------------------------------
// UID / GID resolution
// ---------------------------------------------------------------------------

use anyhow::Context;
use nix::unistd::{Gid, Uid};

/// Resolve the effective UID from an optional username and/or numeric user-id.
///
/// - Both set: look up username, verify it matches user-id.
/// - Only name: look up by name.
/// - Only id: use directly.
/// - Neither: `None` (keep current user).
pub fn resolve_uid(user: Option<&str>, user_id: Option<u32>) -> anyhow::Result<Option<Uid>> {
    match (user, user_id) {
        (Some(name), Some(id)) => {
            let uid = lookup_uid(name)?;
            anyhow::ensure!(
                uid.as_raw() == id,
                "user {:?} has uid {} but user-id {} was specified",
                name,
                uid.as_raw(),
                id
            );
            Ok(Some(uid))
        }
        (Some(name), None) => Ok(Some(lookup_uid(name)?)),
        (None, Some(id)) => Ok(Some(Uid::from_raw(id))),
        (None, None) => Ok(None),
    }
}

/// Resolve the effective GID from an optional group name and/or numeric group-id.
pub fn resolve_gid(group: Option<&str>, group_id: Option<u32>) -> anyhow::Result<Option<Gid>> {
    match (group, group_id) {
        (Some(name), Some(id)) => {
            let gid = lookup_gid(name)?;
            anyhow::ensure!(
                gid.as_raw() == id,
                "group {:?} has gid {} but group-id {} was specified",
                name,
                gid.as_raw(),
                id
            );
            Ok(Some(gid))
        }
        (Some(name), None) => Ok(Some(lookup_gid(name)?)),
        (None, Some(id)) => Ok(Some(Gid::from_raw(id))),
        (None, None) => Ok(None),
    }
}

fn lookup_uid(name: &str) -> anyhow::Result<Uid> {
    nix::unistd::User::from_name(name)
        .with_context(|| format!("getpwnam({:?})", name))?
        .map(|u| u.uid)
        .ok_or_else(|| anyhow::anyhow!("user {:?} not found", name))
}

fn lookup_gid(name: &str) -> anyhow::Result<Gid> {
    nix::unistd::Group::from_name(name)
        .with_context(|| format!("getgrnam({:?})", name))?
        .map(|g| g.gid)
        .ok_or_else(|| anyhow::anyhow!("group {:?} not found", name))
}
