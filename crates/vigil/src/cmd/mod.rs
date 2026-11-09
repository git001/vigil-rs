// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

pub mod alerts;
pub mod checks;
pub mod identities;
pub mod logs;
pub mod services;
pub mod vigild;

/// Print a change result line to stdout (used by services start/stop/restart).
/// A change that ended in `error` is an error of the command (exit code 1),
/// with the reasons as the message.
pub fn print_change(change: &vigil_types::api::ChangeInfo) -> anyhow::Result<()> {
    println!(
        "Change {} [{}]: {}",
        change.id,
        format!("{:?}", change.status).to_lowercase(),
        change.summary,
    );
    match (&change.status, &change.err) {
        (vigil_types::api::ChangeStatus::Error, err) => {
            anyhow::bail!("{}", err.as_deref().unwrap_or("the action failed"))
        }
        (_, Some(err)) => {
            eprintln!("error: {err}");
            Ok(())
        }
        _ => Ok(()),
    }
}
