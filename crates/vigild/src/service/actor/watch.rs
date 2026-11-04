// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `watch-files` in the actor: poll while the process runs, react to a
//! stable change with a restart or a signal.

use std::pin::Pin;

use tokio::time::{Instant, Sleep};
use tracing::{debug, info};
use vigil_types::plan::FileChangeAction;

use crate::service::watch::FileWatch;
use crate::state::ServiceState;
use crate::template::FileRead;

use super::Actor;

impl Actor {
    /// Called after a successful spawn with the files its templates read.
    /// Oneshot jobs are not watched: every run reads its files anew.
    pub(super) async fn start_watching(&mut self, reads: Vec<FileRead>) {
        self.watch = if self.is_oneshot() {
            None
        } else {
            FileWatch::new(&self.name, self.config.watch_files.as_ref(), reads).await
        };
        self.watch_deadline = self.watch.as_ref().map(|w| {
            debug!(service = %self.name, files = ?w.paths(), interval = ?w.interval, "watching files");
            Instant::now() + w.interval
        });
    }

    pub(super) fn stop_watching(&mut self) {
        self.watch = None;
        self.watch_deadline = None;
    }

    pub(super) async fn handle_watch_tick(
        &mut self,
        backoff_sleep: &mut Option<Pin<Box<Sleep>>>,
        stop_deadline: &mut Option<Pin<Box<Sleep>>>,
        pending_restart: &mut bool,
    ) {
        let Some(watch) = self.watch.as_mut() else {
            self.watch_deadline = None;
            return;
        };
        let changed = watch.poll().await;
        let action = watch.action;
        self.watch_deadline = Some(Instant::now() + watch.interval);

        let Some(files) = changed else { return };
        // Only a running process reacts; a stopping one is about to go away.
        if !matches!(self.state, ServiceState::Starting | ServiceState::Active) {
            return;
        }
        info!(
            service = %self.name,
            files = ?files,
            action = %String::from(action),
            "watched file changed"
        );
        self.metrics.record_file_reload(&self.name);
        match action {
            FileChangeAction::Restart => {
                let (tx, _rx) = tokio::sync::oneshot::channel();
                self.handle_restart(tx, backoff_sleep, stop_deadline, pending_restart)
                    .await;
            }
            FileChangeAction::Signal(sig) => self.send_signal(sig.0),
            FileChangeAction::Ignore => {}
        }
    }
}
