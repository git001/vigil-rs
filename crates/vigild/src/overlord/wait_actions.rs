// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `POST /v1/services` answers once the action is done (see
//! [`super::waiting`]): the change is `doing` meanwhile, then `done` or
//! `error`.

use chrono::Utc;
use tokio::sync::oneshot;
use vigil_types::api::{ChangeInfo, ChangeStatus, ServiceAction};

use super::Overlord;
use super::waiting::{Goal, Waiting};
use crate::state::ServiceState;

/// Where the change goes once the action is done.
pub(super) type Reply = oneshot::Sender<anyhow::Result<ChangeInfo>>;

impl Overlord {
    /// Run a service action and answer once it is done. `replan` answers
    /// at once (it is done when the layers are applied).
    pub(super) async fn services_action(
        &mut self,
        action: ServiceAction,
        names: Vec<String>,
        reply: Reply,
    ) {
        let goal = match action {
            ServiceAction::Start | ServiceAction::Restart | ServiceAction::Autostart => Goal::Ready,
            ServiceAction::Stop => Goal::Stopped,
            ServiceAction::Replan => {
                let _ = reply.send(self.handle_services_action(action, names).await);
                return;
            }
        };
        let running_before: Vec<String> = self
            .services
            .iter()
            .filter(|(_, e)| e.snapshot.state.is_running())
            .map(|(n, _)| n.clone())
            .collect();
        let (targets, errors) = self.dispatch_services_action(action, names).await;
        let change = self.record_doing(action, &targets);

        // A service whose command failed already is not waited for; the
        // errors have the form "name: error".
        let waited: Vec<String> = targets
            .iter()
            .filter(|n| !errors.iter().any(|e| e.starts_with(&format!("{n}: "))))
            .cloned()
            .collect();
        let mut waiting = Waiting::new(change.id, goal, waited.clone());
        waiting.errors = errors;
        for name in &waited {
            if action == ServiceAction::Start && running_before.contains(name) {
                waiting.already_started(name);
            }
            match self.services.get(name) {
                Some(e) => {
                    let state = e.snapshot.state;
                    waiting.update(name, state, || state_text(state));
                }
                None => waiting.give_up(name, "unknown service"),
            }
        }
        self.waiting.push((waiting, reply));
        self.give_up_never_starting();
        self.answer_finished();
    }

    /// A queued service that `requires` a service nothing starts (not
    /// started by autostart, not starting now) would be waited for in vain.
    fn give_up_never_starting(&mut self) {
        let stuck: Vec<(String, String)> = self
            .pending_autostart
            .iter()
            .filter_map(|name| {
                let config = self.plan.services.get(name)?;
                let dep = config.requires.iter().find(|d| {
                    self.autostart_skip_reason(d).is_some()
                        && self
                            .services
                            .get(*d)
                            .is_none_or(|e| !e.snapshot.state.is_running())
                })?;
                Some((name.clone(), dep.clone()))
            })
            .collect();
        for (w, _) in &mut self.waiting {
            if w.goal != Goal::Ready {
                continue;
            }
            for (name, dep) in &stuck {
                w.give_up(
                    name,
                    &format!("waits for {dep}, which autostart does not start"),
                );
            }
        }
    }

    /// Feed a state change to the waiting actions; a service that waits for
    /// `service`, which failed for good, will not start.
    pub(super) fn waiting_state_changed(&mut self, service: &str, state: ServiceState) {
        let last_exit = self
            .services
            .get(service)
            .and_then(|e| e.snapshot.last_exit);
        let blocked: Vec<String> = if state == ServiceState::Error {
            self.blocked_by(service)
                .into_iter()
                .map(str::to_owned)
                .collect()
        } else {
            Vec::new()
        };
        for (w, _) in &mut self.waiting {
            w.update(service, state, || match state {
                ServiceState::Inactive => crate::service::describe_exit(last_exit),
                other => state_text(other),
            });
            if w.goal == Goal::Ready {
                for b in &blocked {
                    w.give_up(b, &format!("waits for {service}, which failed"));
                }
            }
        }
        self.answer_finished();
    }

    /// `service` reported why it failed.
    pub(super) fn waiting_unhealthy(&mut self, service: &str, message: &str) {
        for (w, _) in &mut self.waiting {
            w.unhealthy(service, message);
        }
    }

    /// After a replan: services gone from the plan are not waited for.
    pub(super) fn waiting_plan_changed(&mut self) {
        for (w, _) in &mut self.waiting {
            let gone: Vec<String> = w
                .pending()
                .iter()
                .filter(|n| !self.services.contains_key(*n))
                .cloned()
                .collect();
            for name in gone {
                w.give_up(&name, "removed from the plan");
            }
        }
        self.answer_finished();
    }

    /// Answer the actions that are done, and record them.
    fn answer_finished(&mut self) {
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|(w, _)| w.finished());
        self.waiting = waiting;
        for (w, reply) in done {
            let Some(change) = self.changes.iter_mut().find(|c| c.id == w.change_id) else {
                continue;
            };
            change.ready_time = Some(Utc::now());
            if w.errors.is_empty() {
                change.status = ChangeStatus::Done;
            } else {
                change.status = ChangeStatus::Error;
                change.err = Some(w.errors.join("; "));
            }
            let _ = reply.send(Ok(change.clone()));
        }
    }

    /// Record a change that is still running.
    fn record_doing(&mut self, action: ServiceAction, targets: &[String]) -> ChangeInfo {
        let mut change = self.record_change(action, targets, Vec::new());
        change.status = ChangeStatus::Doing;
        change.ready_time = None;
        if let Some(c) = self.changes.iter_mut().find(|c| c.id == change.id) {
            *c = change.clone();
        }
        change
    }
}

/// A state as a failure reason.
fn state_text(state: ServiceState) -> String {
    format!(
        "is {}",
        format!("{:?}", state.to_api_status()).to_lowercase()
    )
}
