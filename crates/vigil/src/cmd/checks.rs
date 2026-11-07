// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use clap::Subcommand;
use vigil_client::client::VigilClient;

#[derive(Debug, Subcommand)]
pub enum ChecksCmd {
    /// List health checks and their current status.
    #[command(name = "list")]
    List {
        /// Filter by check name(s).
        names: Vec<String>,
    },
}

pub async fn run(client: &VigilClient, sub: ChecksCmd) -> anyhow::Result<()> {
    let ChecksCmd::List { names } = sub;
    let checks = client.list_checks(&names).await?;
    if checks.is_empty() {
        println!("No checks.");
        return Ok(());
    }
    println!(
        "{:<24} {:<8} {:<6} {:<10} {:<10} Last error",
        "Check", "Level", "Status", "Failures", "Next run"
    );
    println!("{}", "-".repeat(90));
    for chk in checks {
        let next = match chk.next_run_in_secs {
            None => "pending".to_string(),
            Some(s) if s < 60 => format!("{}s", s),
            Some(s) if s < 3600 => format!("{}m {}s", s / 60, s % 60),
            Some(s) => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        };
        let line = format!(
            "{:<24} {:<8} {:<6} {:<10} {:<10} {}",
            chk.name,
            format!("{:?}", chk.level).to_lowercase(),
            format!("{:?}", chk.status).to_lowercase(),
            format!("{}/{}", chk.failures, chk.threshold),
            next,
            chk.last_error
                .as_deref()
                .map(short_error)
                .unwrap_or_default(),
        );
        println!("{}", line.trim_end());
    }
    Ok(())
}

/// Longest `Last error` shown in the table; `GET /v1/checks` has the full
/// text.
const ERROR_WIDTH: usize = 60;

/// The first line of `error`, cut to [`ERROR_WIDTH`] characters; `…` marks
/// a cut.
fn short_error(error: &str) -> String {
    let mut lines = error.lines();
    let first = lines.next().unwrap_or_default();
    let more = lines.next().is_some();
    if first.chars().count() > ERROR_WIDTH {
        let cut: String = first.chars().take(ERROR_WIDTH - 1).collect();
        format!("{cut}…")
    } else if more {
        format!("{first} …")
    } else {
        first.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::short_error;

    #[test]
    fn short_error_keeps_a_short_line() {
        assert_eq!(
            short_error("HTTP 503 Service Unavailable"),
            "HTTP 503 Service Unavailable"
        );
    }

    #[test]
    fn short_error_shows_only_the_first_line() {
        assert_eq!(
            short_error("exit status 1: first\nsecond"),
            "exit status 1: first …"
        );
    }

    #[test]
    fn short_error_cuts_a_long_line() {
        let long = "x".repeat(100);
        let short = short_error(&long);
        assert_eq!(short.chars().count(), 60);
        assert!(short.ends_with('…'));
    }
}
