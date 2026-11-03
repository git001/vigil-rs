// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! sd_notify receiver for `ready: notify` services.
//!
//! Every start of such a service gets its own datagram socket
//! `<path-prefix>/notify/<service>-<64-bit random>.sock` (mode 0600, owned by
//! the service user). Its path is passed in `NOTIFY_SOCKET`. The socket is
//! removed when the process exits, so a leftover process of an earlier start
//! cannot report into a later one.
//!
//! A filesystem path (not an abstract socket) is used on purpose: abstract
//! names live in the network namespace and can be squatted by any process in
//! the pod.
//!
//! Each datagram is read with `SCM_CREDENTIALS`; the sender's PID, UID and
//! process group are passed on to the actor, which decides whether the
//! sender belongs to the service (see [`NotifyMsg::is_from`]).

use std::io::IoSliceMut;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use nix::sys::socket::{
    ControlMessageOwned, MsgFlags, UnixCredentials, recvmsg, setsockopt, sockopt,
};
use nix::unistd::{Gid, Pid, Uid};
use tokio::io::Interest;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Longest path that fits into `sockaddr_un.sun_path` (108 bytes incl. NUL).
const SUN_PATH_MAX: usize = 107;

/// Largest datagram we accept; longer messages are truncated and dropped.
const MAX_DATAGRAM: usize = 4096;

/// One datagram received on a notify socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyMsg {
    /// Start generation of the socket that received it.
    pub generation: u64,
    pub pid: i32,
    pub uid: u32,
    /// Process group of the sender at receive time; `None` if it has
    /// already exited.
    pub pgid: Option<i32>,
    /// `KEY=VALUE` assignments, in order.
    pub fields: Vec<(String, String)>,
}

impl NotifyMsg {
    /// Whether the message comes from the service whose main process is
    /// `main_pid` and which runs as `service_uid`.
    ///
    /// Accepted: the main process itself or any process in its process group
    /// (services are spawned as group leaders). A sender that has already
    /// exited (e.g. a short-lived `systemd-notify` helper) cannot be looked
    /// up any more; it is accepted only if it ran as the service's UID.
    pub fn is_from(&self, main_pid: i32, service_uid: u32) -> bool {
        match self.pgid {
            _ if self.pid == main_pid => true,
            Some(pgid) => pgid == main_pid,
            None => self.uid == service_uid,
        }
    }

    /// Value of the last assignment to `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Parse an sd_notify datagram: newline-separated `KEY=VALUE` lines. Lines
/// without `=` are ignored.
pub fn parse(payload: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(payload)
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A bound notify socket and its receiver task. Dropping it stops the task
/// and removes the socket file.
pub struct NotifySocket {
    path: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl NotifySocket {
    /// Bind a fresh socket in `dir` for `service`.
    ///
    /// `owner` is the service's UID/GID when it runs as a different user;
    /// the socket is then chowned to it (only possible as root).
    pub fn bind(
        dir: &Path,
        service: &str,
        owner: Option<(Uid, Gid)>,
        generation: u64,
        tx: mpsc::Sender<NotifyMsg>,
    ) -> anyhow::Result<Self> {
        let path = socket_path(dir, service, &random_suffix());
        let std_sock = std::os::unix::net::UnixDatagram::bind(&path)
            .with_context(|| format!("binding notify socket {path:?}"))?;
        // From here on, `guard` removes the file on any error.
        let guard = RemoveOnDrop(Some(path.clone()));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 600 {path:?}"))?;
        if let Some((uid, gid)) = owner
            && uid != Uid::effective()
        {
            nix::unistd::chown(&path, Some(uid), Some(gid))
                .with_context(|| format!("chown {uid}:{gid} {path:?}"))?;
        }
        setsockopt(&std_sock, sockopt::PassCred, &true).context("SO_PASSCRED")?;
        std_sock.set_nonblocking(true)?;
        let sock = tokio::net::UnixDatagram::from_std(std_sock)?;

        let task = tokio::spawn(receive_loop(sock, generation, tx));
        let path = guard.disarm();
        Ok(NotifySocket { path, task })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for NotifySocket {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

struct RemoveOnDrop(Option<PathBuf>);

impl RemoveOnDrop {
    fn disarm(mut self) -> PathBuf {
        self.0.take().expect("armed")
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn random_suffix() -> String {
    let n = uuid::Uuid::new_v4().as_u128() as u64;
    format!("{n:016x}")
}

/// `<dir>/<service>-<rand>.sock`, or `<dir>/<rand>.sock` if that would not
/// fit into `sun_path` (long service names).
fn socket_path(dir: &Path, service: &str, rand: &str) -> PathBuf {
    let named = dir.join(format!("{service}-{rand}.sock"));
    if named.as_os_str().len() <= SUN_PATH_MAX {
        named
    } else {
        dir.join(format!("{rand}.sock"))
    }
}

async fn receive_loop(
    sock: tokio::net::UnixDatagram,
    generation: u64,
    tx: mpsc::Sender<NotifyMsg>,
) {
    loop {
        if let Err(e) = sock.readable().await {
            warn!(error = %e, "notify socket failed");
            return;
        }
        let res = sock.try_io(Interest::READABLE, || recv_one(sock.as_raw_fd()));
        let (payload, creds) = match res {
            Ok(Some(m)) => m,
            Ok(None) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                debug!(error = %e, "notify recvmsg failed");
                continue;
            }
        };
        let Some(creds) = creds else {
            debug!("notify datagram without credentials dropped");
            continue;
        };
        let pgid = nix::unistd::getpgid(Some(Pid::from_raw(creds.pid())))
            .ok()
            .map(Pid::as_raw);
        let msg = NotifyMsg {
            generation,
            pid: creds.pid(),
            uid: creds.uid(),
            pgid,
            fields: parse(&payload),
        };
        if tx.send(msg).await.is_err() {
            return;
        }
    }
}

/// Receive one datagram. `Ok(None)` for a datagram that was dropped
/// (truncated payload or control data).
fn recv_one(fd: RawFd) -> std::io::Result<Option<(Vec<u8>, Option<UnixCredentials>)>> {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    let mut cmsg = nix::cmsg_space!(UnixCredentials, [RawFd; 16]);
    let mut iov = [IoSliceMut::new(&mut buf)];
    let msg = recvmsg::<()>(
        fd,
        &mut iov,
        Some(&mut cmsg),
        MsgFlags::MSG_TRUNC | MsgFlags::MSG_CMSG_CLOEXEC,
    )
    .map_err(std::io::Error::from)?;
    let len = msg.bytes;
    if msg.flags.contains(MsgFlags::MSG_TRUNC) || msg.flags.contains(MsgFlags::MSG_CTRUNC) {
        debug!(len, "truncated notify datagram dropped");
        return Ok(None);
    }
    let mut creds = None;
    for c in msg.cmsgs().map_err(std::io::Error::from)? {
        match c {
            ControlMessageOwned::ScmCredentials(c) => creds = Some(c),
            // File descriptors (FDSTORE=1) are not supported: close them.
            ControlMessageOwned::ScmRights(fds) => {
                for fd in fds {
                    let _ = nix::unistd::close(fd);
                }
            }
            _ => {}
        }
    }
    buf.truncate(len);
    Ok(Some((buf, creds)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn parse_splits_lines_and_ignores_garbage() {
        let f = parse(b"READY=1\nSTATUS=warming up: 3/5\ngarbage\nMAINPID=42\n");
        assert_eq!(
            f,
            vec![
                ("READY".into(), "1".into()),
                ("STATUS".into(), "warming up: 3/5".into()),
                ("MAINPID".into(), "42".into()),
            ]
        );
    }

    fn msg(pid: i32, uid: u32, pgid: Option<i32>) -> NotifyMsg {
        NotifyMsg {
            generation: 1,
            pid,
            uid,
            pgid,
            fields: vec![("STATUS".into(), "a".into()), ("STATUS".into(), "b".into())],
        }
    }

    #[test]
    fn get_returns_last_assignment() {
        assert_eq!(msg(1, 0, None).get("STATUS"), Some("b"));
        assert_eq!(msg(1, 0, None).get("READY"), None);
    }

    #[test]
    fn sender_checks() {
        // main process
        assert!(msg(100, 5, Some(100)).is_from(100, 9));
        // child in the service's process group
        assert!(msg(101, 5, Some(100)).is_from(100, 9));
        // other process group, even with the same uid
        assert!(!msg(200, 9, Some(200)).is_from(100, 9));
        // exited sender: uid decides
        assert!(msg(101, 9, None).is_from(100, 9));
        assert!(!msg(101, 5, None).is_from(100, 9));
    }

    #[test]
    fn long_service_names_fall_back_to_random_only() {
        let dir = Path::new("/tmp/vigil/notify");
        let short = socket_path(dir, "web", "0123456789abcdef");
        assert_eq!(
            short,
            Path::new("/tmp/vigil/notify/web-0123456789abcdef.sock")
        );
        let long = socket_path(dir, &"x".repeat(100), "0123456789abcdef");
        assert_eq!(long, Path::new("/tmp/vigil/notify/0123456789abcdef.sock"));
    }

    #[tokio::test]
    async fn receives_datagram_with_credentials_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        let sock = NotifySocket::bind(dir.path(), "svc", None, 7, tx).unwrap();
        let path = sock.path().to_path_buf();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let client = std::os::unix::net::UnixDatagram::unbound().unwrap();
        client.send_to(b"READY=1\nSTATUS=up\n", &path).unwrap();
        let m = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(m.generation, 7);
        assert_eq!(m.pid, std::process::id() as i32);
        assert_eq!(m.uid, Uid::effective().as_raw());
        assert_eq!(m.get("READY"), Some("1"));
        assert!(m.is_from(m.pgid.unwrap(), 0));

        drop(sock);
        assert!(!path.exists(), "socket file removed on drop");
    }
}
