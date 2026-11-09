// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Service actions that answer once they are done (`POST /v1/services`):
//! `start`, `restart` and `autostart` once the services are ready, `stop`
//! once their processes are gone.
//!
//! The overlord does not wait itself: readiness (`ready: check`) comes in
//! through its own event loop. It keeps a [`Waiting`] per action and feeds
//! it every state change; the reply goes out when the last service is done.

use std::collections::{HashMap, HashSet};

use crate::state::ServiceState;

/// What an action waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Goal {
    /// Ready: `active`, or `done` for a oneshot. A service counts only after
    /// it was `starting` since the action began, so the state of an earlier
    /// run (an `active` before a restart, a `done` of the last run) does not.
    Ready,
    /// No process left: `inactive`, `error` or `done`.
    Stopped,
}

/// The outcome for one service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Pending,
    Done,
    Failed,
}

/// One action that waits for its services.
#[derive(Debug)]
pub(super) struct Waiting {
    pub(super) change_id: String,
    pub(super) goal: Goal,
    /// Services not done yet.
    pending: Vec<String>,
    /// Services seen `starting` since the action began (for [`Goal::Ready`]).
    started: HashSet<String>,
    /// The first unhealthy message of each service since the action began:
    /// why it failed ("did not become ready within 30s", "exited with
    /// code 1, restarting in 1s").
    reasons: HashMap<String, String>,
    pub(super) errors: Vec<String>,
}

impl Waiting {
    pub(super) fn new(change_id: String, goal: Goal, names: Vec<String>) -> Self {
        Waiting {
            change_id,
            goal,
            pending: names,
            started: HashSet::new(),
            reasons: HashMap::new(),
            errors: Vec::new(),
        }
    }

    /// For a `start` of a service that already runs: it counts as started.
    pub(super) fn already_started(&mut self, name: &str) {
        self.started.insert(name.to_owned());
    }

    /// Whether every service is done (or failed).
    pub(super) fn finished(&self) -> bool {
        self.pending.is_empty()
    }

    /// The services still waited for.
    pub(super) fn pending(&self) -> &[String] {
        &self.pending
    }

    /// `name` reported a failure. vigild sends it before the state it
    /// leads to, so the reason is known when the state comes.
    pub(super) fn unhealthy(&mut self, name: &str, message: &str) {
        if self.pending.iter().any(|n| n == name) {
            self.reasons
                .entry(name.to_owned())
                .or_insert_with(|| message.to_owned());
        }
    }

    /// `name` is in `state` now. If that is a failure without an unhealthy
    /// message (a clean exit while it starts), `fallback` says why.
    pub(super) fn update(
        &mut self,
        name: &str,
        state: ServiceState,
        fallback: impl FnOnce() -> String,
    ) {
        if !self.pending.iter().any(|n| n == name) {
            return;
        }
        match self.outcome(name, state) {
            Outcome::Pending => return,
            Outcome::Done => {}
            Outcome::Failed => {
                let why = self.reasons.remove(name).unwrap_or_else(fallback);
                self.errors.push(format!("{name}: {why}"));
            }
        }
        self.pending.retain(|n| n != name);
    }

    /// `name` cannot get there: it is gone from the plan, or it waits for a
    /// dependency that failed.
    pub(super) fn give_up(&mut self, name: &str, reason: &str) {
        if self.pending.iter().any(|n| n == name) {
            self.errors.push(format!("{name}: {reason}"));
            self.pending.retain(|n| n != name);
        }
    }

    fn outcome(&mut self, name: &str, state: ServiceState) -> Outcome {
        match self.goal {
            Goal::Stopped => {
                if state.has_process() || state == ServiceState::Backoff {
                    Outcome::Pending
                } else {
                    Outcome::Done
                }
            }
            Goal::Ready => {
                if state == ServiceState::Starting {
                    self.started.insert(name.to_owned());
                    return Outcome::Pending;
                }
                if !self.started.contains(name) {
                    return Outcome::Pending;
                }
                match state {
                    ServiceState::Active | ServiceState::Done => Outcome::Done,
                    ServiceState::Starting | ServiceState::Stopping => Outcome::Pending,
                    ServiceState::Backoff | ServiceState::Error | ServiceState::Inactive => {
                        Outcome::Failed
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ServiceState::*;

    fn why() -> String {
        "exited with code 1".into()
    }

    #[test]
    fn start_waits_for_ready_after_starting() {
        let mut w = Waiting::new("1".into(), Goal::Ready, vec!["a".into()]);
        w.update("a", Done, why);
        assert!(!w.finished(), "a done of an earlier run does not count");
        w.update("a", Starting, why);
        assert!(!w.finished());
        w.update("a", Active, why);
        assert!(w.finished());
        assert!(w.errors.is_empty());
    }

    #[test]
    fn restart_ignores_the_old_active_and_the_stop() {
        let mut w = Waiting::new("1".into(), Goal::Ready, vec!["a".into()]);
        for s in [Active, Stopping, Inactive] {
            w.update("a", s, why);
        }
        assert!(!w.finished());
        w.update("a", Starting, why);
        w.update("a", Active, why);
        assert!(w.finished());
    }

    #[test]
    fn a_start_that_fails_is_an_error() {
        let mut w = Waiting::new("1".into(), Goal::Ready, vec!["a".into(), "b".into()]);
        w.update("a", Starting, why);
        w.unhealthy("a", "exited with code 3, restarting in 1s");
        w.update("a", Backoff, why);
        assert_eq!(w.pending(), ["b"]);
        w.update("b", Starting, why);
        w.unhealthy("b", "did not become ready within 1s");
        w.unhealthy("b", "exited with signal 15, restarting in 1s");
        w.update("b", Backoff, why);
        // c exits cleanly while it starts: no message, the exit says it.
        assert!(w.finished());
        assert_eq!(
            w.errors,
            [
                "a: exited with code 3, restarting in 1s",
                "b: did not become ready within 1s"
            ]
        );
        let mut w = Waiting::new("2".into(), Goal::Ready, vec!["c".into()]);
        w.update("c", Starting, why);
        w.update("c", Inactive, || "exited with code 0".into());
        assert_eq!(w.errors, ["c: exited with code 0"]);
    }

    #[test]
    fn a_running_service_is_ready_at_once() {
        let mut w = Waiting::new("1".into(), Goal::Ready, vec!["a".into()]);
        w.already_started("a");
        w.update("a", Active, why);
        assert!(w.finished());
    }

    #[test]
    fn stop_waits_until_no_process_is_left() {
        let mut w = Waiting::new("1".into(), Goal::Stopped, vec!["a".into()]);
        w.update("a", Active, why);
        w.update("a", Stopping, why);
        assert!(!w.finished());
        w.update("a", Inactive, why);
        assert!(w.finished());
        assert!(w.errors.is_empty());
    }

    #[test]
    fn give_up_reports_the_reason() {
        let mut w = Waiting::new("1".into(), Goal::Ready, vec!["a".into()]);
        w.give_up("a", "waits for db, which failed");
        assert!(w.finished());
        assert_eq!(w.errors, ["a: waits for db, which failed"]);
        w.give_up("a", "again");
        assert_eq!(w.errors.len(), 1);
    }
}
