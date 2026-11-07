// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! A notify datagram: its `KEY=VALUE` fields and who sent it.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
