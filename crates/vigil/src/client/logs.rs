// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `GET /v1/logs` and `GET /v1/logs/follow`.

use vigil_types::api::LogEntry;

use super::VigilClient;
use super::transport::drain_sse_buf;

impl VigilClient {
    pub async fn list_logs(
        &self,
        services: &[String],
        n: Option<usize>,
    ) -> anyhow::Result<Vec<LogEntry>> {
        let mut params = Vec::new();
        if !services.is_empty() {
            params.push(format!("services={}", services.join(",")));
        }
        if let Some(n) = n {
            params.push(format!("n={}", n));
        }
        let query = if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        };
        self.get(&format!("/v1/logs{}", query)).await
    }

    /// Stream live log entries (SSE) — prints to stdout until EOF or Ctrl-C.
    pub async fn follow_logs(&self, services: &[String]) -> anyhow::Result<()> {
        let path = if services.is_empty() {
            "/v1/logs/follow".to_string()
        } else {
            format!("/v1/logs/follow?services={}", services.join(","))
        };

        let mut buf = String::new();
        self.transport
            .stream(&path, "text/event-stream", |chunk| {
                buf.push_str(&String::from_utf8_lossy(chunk));
                drain_sse_buf(&mut buf);
            })
            .await
    }
}
