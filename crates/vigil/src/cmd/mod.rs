// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

pub mod checks;
pub mod identities;
pub mod logs;
pub mod services;
pub mod vigild;

/// Print a change result line to stdout (used by services start/stop/restart).
pub fn print_change(change: &vigil_types::api::ChangeInfo) {
    println!(
        "Change {} [{}]: {}",
        change.id,
        format!("{:?}", change.status).to_lowercase(),
        change.summary,
    );
    if let Some(err) = &change.err {
        eprintln!("error: {}", err);
    }
}
