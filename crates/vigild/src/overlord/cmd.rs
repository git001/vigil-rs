// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! What the API sends the overlord.

use tokio::sync::{mpsc, oneshot};
use vigil_types::api::{AlertInfo, ChangeInfo, CheckInfo, ServiceInfo, SystemInfo};

pub enum Cmd {
    Services {
        action: vigil_types::api::ServiceAction,
        names: Vec<String>,
        reply: oneshot::Sender<anyhow::Result<ChangeInfo>>,
    },
    GetServices {
        names: Vec<String>,
        reply: oneshot::Sender<Vec<ServiceInfo>>,
    },
    GetChanges {
        id: Option<String>,
        reply: oneshot::Sender<Vec<ChangeInfo>>,
    },
    GetChecks {
        names: Vec<String>,
        reply: oneshot::Sender<Vec<CheckInfo>>,
    },
    /// Run a check now (`POST /v1/checks/refresh`). `None`: no check of
    /// that name in the plan. The overlord only hands the request on; the
    /// reply comes from the check, so a slow probe never blocks it.
    RefreshCheck {
        name: String,
        reply: oneshot::Sender<Option<vigil_types::api::CheckRefresh>>,
    },
    /// `GET /v1/health`: whether no selected, running check is down. `Err`
    /// names a check that is not in the plan.
    Health {
        level: Option<vigil_types::plan::CheckLevel>,
        names: Vec<String>,
        reply: oneshot::Sender<Result<bool, String>>,
    },
    GetAlerts {
        names: Vec<String>,
        reply: oneshot::Sender<Vec<AlertInfo>>,
    },
    GetSystemInfo {
        reply: oneshot::Sender<SystemInfo>,
    },
    ReloadLayers {
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    /// Send `signal` to the process groups of `services`
    /// (`POST /v1/signals`). All of them must exist and run, otherwise
    /// none gets it. Ok: the services that got it.
    Signal {
        signal: nix::sys::signal::Signal,
        services: Vec<String>,
        reply: oneshot::Sender<Result<Vec<String>, SignalError>>,
    },
    /// Forward a signal to all currently-running service process groups.
    ForwardSignal {
        signal: nix::sys::signal::Signal,
    },
    Shutdown,
}

/// Why `POST /v1/signals` sent nothing (or not to every service).
#[derive(Debug, PartialEq)]
pub enum SignalError {
    /// No service named in the request (400).
    NoServices,
    /// A name that is not a service (404).
    NotFound(String),
    /// Services without a running process (409).
    NotRunning(Vec<String>),
}

impl std::fmt::Display for SignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoServices => write!(f, "services must name at least one service"),
            Self::NotFound(name) => write!(f, "service {name:?} not found"),
            Self::NotRunning(names) => {
                let list: Vec<String> = names.iter().map(|n| format!("{n:?}")).collect();
                write!(f, "not running: {}", list.join(", "))
            }
        }
    }
}

#[derive(Clone)]
pub struct Handle {
    pub tx: mpsc::Sender<Cmd>,
}
