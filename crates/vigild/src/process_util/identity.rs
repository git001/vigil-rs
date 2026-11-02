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

// ---------------------------------------------------------------------------
// Full credential set for a privilege drop
// ---------------------------------------------------------------------------

/// Everything a child needs to switch identity, resolved *before* `fork` so
/// that the `pre_exec` hook only performs async-signal-safe syscalls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub uid: Uid,
    pub gid: Gid,
    /// Complete supplementary group list (always contains `gid`).
    pub groups: Vec<Gid>,
}

/// Resolve the complete identity a child process should run as.
///
/// Returns `None` when neither user nor group is configured: the child then
/// keeps vigild's own identity and no credential syscall is made at all.
/// This is the normal case on OpenShift (random UID, no capabilities), where
/// any `setgroups`/`setgid` call would fail with `EPERM`.
///
/// When a user is configured but no group, the user's primary group from the
/// passwd database is used. Previously the GID was left untouched, so a child
/// dropped from root kept GID 0 and all of root's supplementary groups.
pub fn resolve_credentials(
    user: Option<&str>,
    user_id: Option<u32>,
    group: Option<&str>,
    group_id: Option<u32>,
) -> anyhow::Result<Option<Credentials>> {
    let uid = resolve_uid(user, user_id)?;
    let gid = resolve_gid(group, group_id)?;
    if uid.is_none() && gid.is_none() {
        return Ok(None);
    }

    let uid = uid.unwrap_or_else(Uid::effective);
    let pw = nix::unistd::User::from_uid(uid).with_context(|| format!("getpwuid({uid})"))?;

    let gid = match (gid, &pw) {
        (Some(g), _) => g,
        (None, Some(pw)) => pw.gid,
        (None, None) => {
            // v3.x compatibility: warn and fall back to gid = uid instead of
            // failing (v4 makes this an error). Keeping vigild's GID, as
            // before 3.2.2, would leave the child in group 0.
            tracing::warn!(
                uid = uid.as_raw(),
                "uid has no passwd entry and no group is configured; using gid = uid. \
                 Set `group` or `group-id` explicitly (this becomes an error in v4)"
            );
            Gid::from_raw(uid.as_raw())
        }
    };

    let mut groups = match &pw {
        Some(pw) => {
            let name = std::ffi::CString::new(pw.name.as_bytes())
                .with_context(|| format!("user name {:?} contains NUL", pw.name))?;
            nix::unistd::getgrouplist(&name, gid)
                .with_context(|| format!("getgrouplist({:?})", pw.name))?
        }
        None => Vec::new(),
    };
    if !groups.contains(&gid) {
        groups.insert(0, gid);
    }

    Ok(Some(Credentials { uid, gid, groups }))
}

impl Credentials {
    /// True when vigild runs unprivileged and already has this uid/gid. The
    /// child then inherits the identity as-is: an unprivileged process could
    /// not call `setgroups` anyway (EPERM), and rootless setups that name
    /// their own user must keep working.
    fn is_current_unprivileged(&self) -> bool {
        let euid = Uid::effective();
        !euid.is_root() && self.uid == euid && self.gid == Gid::effective()
    }
}

/// Install a `pre_exec` hook on `cmd` that switches to `creds`.
///
/// Order matters: supplementary groups and GID must be set while still
/// privileged, the UID last. Afterwards we verify that regaining root is
/// impossible, so a partially failed drop can never start the child.
pub fn apply_credentials(cmd: &mut tokio::process::Command, creds: Option<Credentials>) {
    let Some(creds) = creds else { return };
    if creds.is_current_unprivileged() {
        return;
    }
    let Credentials { uid, gid, groups } = creds;
    // SAFETY: the closure runs between fork and exec of a multi-threaded
    // process. It only calls setgroups/setgid/setuid (async-signal-safe) on
    // data resolved before the fork and never allocates, not even on the
    // error path (std reports only the errno to the parent anyway).
    unsafe {
        cmd.pre_exec(move || {
            let os = |e: nix::errno::Errno| std::io::Error::from_raw_os_error(e as i32);
            nix::unistd::setgroups(&groups).map_err(os)?;
            nix::unistd::setgid(gid).map_err(os)?;
            nix::unistd::setuid(uid).map_err(os)?;
            if !uid.is_root() && nix::unistd::setuid(Uid::from_raw(0)).is_ok() {
                return Err(std::io::Error::from_raw_os_error(nix::libc::EPERM));
            }
            Ok(())
        });
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
