// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! The output of an exec check: one pipe for stdout and stderr, its last
//! 4 KiB, and the exit reason.

use std::os::fd::OwnedFd;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::unix::pipe;

/// How much of a failed exec check's output is kept: its last 4 KiB.
const OUTPUT_TAIL: usize = 4096;
/// How long output is still read after the command exited.
pub(super) const OUTPUT_DRAIN: Duration = Duration::from_millis(200);

/// The read end of the output pipe and two write ends (stdout, stderr).
pub(super) fn pipe_pair() -> std::io::Result<(pipe::Receiver, (Stdio, Stdio))> {
    let (r, w) = std::io::pipe()?;
    let w2 = w.try_clone()?;
    let reader = pipe::Receiver::from_owned_fd(OwnedFd::from(r))?;
    Ok((reader, (Stdio::from(w), Stdio::from(w2))))
}

/// Wait for the child and read its output meanwhile. Sets `reader` to
/// `None` once the pipe reported EOF or an error.
pub(super) async fn wait_and_read(
    child: &mut tokio::process::Child,
    spawned_at: std::time::Instant,
    reader: &mut Option<pipe::Receiver>,
    output: &mut OutputTail,
) -> std::io::Result<std::process::ExitStatus> {
    let wait = crate::reaper::wait_child(child, spawned_at);
    tokio::pin!(wait);
    let mut buf = [0u8; 1024];
    loop {
        let Some(r) = reader.as_mut() else {
            return wait.await;
        };
        tokio::select! {
            status = &mut wait => return status,
            n = r.read(&mut buf) => match n {
                Ok(0) | Err(_) => *reader = None,
                Ok(n) => output.push(&buf[..n]),
            },
        }
    }
}

/// `exit status 2`, or `killed by signal 9`.
pub(super) fn exit_reason(status: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit status {code}"),
        (None, Some(sig)) => format!("killed by signal {sig}"),
        (None, None) => format!("{status}"),
    }
}

/// The last [`OUTPUT_TAIL`] bytes of a command's output.
#[derive(Default)]
pub(super) struct OutputTail {
    buf: Vec<u8>,
    truncated: bool,
}

impl OutputTail {
    pub(super) fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        if self.buf.len() > OUTPUT_TAIL {
            let cut = self.buf.len() - OUTPUT_TAIL;
            self.buf.drain(..cut);
            self.truncated = true;
        }
    }

    pub(super) async fn read_to_end(&mut self, r: &mut pipe::Receiver) {
        let mut buf = [0u8; 1024];
        while let Ok(n @ 1..) = r.read(&mut buf).await {
            self.push(&buf[..n]);
        }
    }

    /// `reason: output`, the output trimmed; `reason` alone without output.
    pub(super) fn append_to(&self, reason: String) -> String {
        let text = String::from_utf8_lossy(&self.buf);
        let text = text.trim();
        match (text.is_empty(), self.truncated) {
            (true, _) => reason,
            (false, false) => format!("{reason}: {text}"),
            (false, true) => format!("{reason}: …{text}"),
        }
    }
}
