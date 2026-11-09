// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Overlord command dispatch: handles `Cmd` messages from the HTTP API.

use vigil_types::api::SystemInfo;

use super::{Cmd, Overlord};

impl Overlord {
    pub(super) async fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Services {
                action,
                names,
                reply,
            } => {
                self.services_action(action, names, reply).await;
            }
            Cmd::GetServices { names, reply } => {
                let _ = reply.send(self.get_service_infos(&names));
            }
            Cmd::GetChanges { id, reply } => {
                let result = match id {
                    None => self.changes.clone(),
                    Some(ref id) => self
                        .changes
                        .iter()
                        .filter(|c| &c.id == id)
                        .cloned()
                        .collect(),
                };
                let _ = reply.send(result);
            }
            Cmd::GetChecks { names, reply } => {
                let infos = self.query_checks(&names).await;
                let _ = reply.send(infos);
            }
            Cmd::RefreshCheck { name, reply } => self.refresh_check(name, reply),
            Cmd::Signal {
                signal,
                services,
                reply,
            } => self.signal_services(signal, services, reply),
            Cmd::Health {
                level,
                names,
                reply,
            } => {
                let _ = reply.send(self.health(level, &names).await);
            }
            Cmd::GetAlerts { names, reply } => {
                let infos = self.query_alerts(&names);
                let _ = reply.send(infos);
            }
            Cmd::GetSystemInfo { reply } => {
                let _ = reply.send(SystemInfo {
                    boot_id: self.boot_id.clone(),
                    start_time: self.start_time,
                    http_address: self.http_address.clone(),
                    https_address: self.https_address.clone(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                });
            }
            Cmd::ReloadLayers { reply } => {
                let _ = reply.send(self.reload_layers().await);
            }
            Cmd::ForwardSignal { signal } => {
                self.forward_signal(signal).await;
            }
            Cmd::Shutdown => unreachable!(),
        }
    }
}
