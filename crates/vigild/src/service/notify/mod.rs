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

mod msg;
mod path;
pub use msg::{NotifyMsg, parse};
pub use path::check_dir;
use path::{RemoveOnDrop, random_suffix, socket_path};

/// Largest datagram we accept; longer messages are truncated and dropped.
const MAX_DATAGRAM: usize = 4096;

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

    async fn next_msg(rx: &mut mpsc::Receiver<NotifyMsg>) -> NotifyMsg {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn oversized_datagrams_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        let sock = NotifySocket::bind(dir.path(), "svc", None, 1, tx).unwrap();
        let client = std::os::unix::net::UnixDatagram::unbound().unwrap();

        let mut big = b"STATUS=".to_vec();
        big.resize(MAX_DATAGRAM + 1, b'x');
        client.send_to(&big, sock.path()).unwrap();
        client.send_to(b"READY=1\n", sock.path()).unwrap();

        let m = next_msg(&mut rx).await;
        assert_eq!(m.get("STATUS"), None, "the truncated one never arrives");
        assert_eq!(m.get("READY"), Some("1"));
    }

    #[tokio::test]
    async fn passed_file_descriptors_are_closed() {
        use nix::sys::socket::{ControlMessage, sendmsg};
        use std::io::Read;
        use std::os::fd::AsRawFd;

        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        let sock = NotifySocket::bind(dir.path(), "svc", None, 1, tx).unwrap();
        let client = std::os::unix::net::UnixDatagram::unbound().unwrap();
        client.connect(sock.path()).unwrap();

        // FDSTORE=1 with the write end of a pipe: once vigild closes its
        // copy and we close ours, the read end sees EOF.
        let (mut read_end, write_end) = std::io::pipe().unwrap();
        let fds = [write_end.as_raw_fd()];
        sendmsg::<()>(
            client.as_raw_fd(),
            &[std::io::IoSlice::new(b"FDSTORE=1\nSTATUS=fd\n")],
            &[ControlMessage::ScmRights(&fds)],
            MsgFlags::empty(),
            None,
        )
        .unwrap();
        let m = next_msg(&mut rx).await;
        assert_eq!(m.get("STATUS"), Some("fd"), "the message itself counts");

        drop(write_end);
        // Poll without blocking: a blocking read could not be cancelled
        // and would hang the test if the descriptor stayed open.
        let flags = nix::fcntl::fcntl(read_end.as_raw_fd(), nix::fcntl::FcntlArg::F_GETFL).unwrap();
        let flags = nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK;
        nix::fcntl::fcntl(read_end.as_raw_fd(), nix::fcntl::FcntlArg::F_SETFL(flags)).unwrap();
        let mut buf = [0u8; 1];
        for _ in 0..100 {
            match read_end.read(&mut buf) {
                Ok(0) => return, // EOF: vigild closed its copy
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                other => panic!("unexpected read result {other:?}"),
            }
        }
        panic!("vigild kept the passed descriptor open");
    }
}
