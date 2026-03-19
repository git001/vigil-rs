//! HTTP/HTTPS URL source — connects to a streaming endpoint via reqwest.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::TryStreamExt;
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::io::StreamReader;
use tracing::{info, warn};

use crate::{LineFilter, Liveness, ReconnectConfig, SourceConnConfig};
use crate::source_http::{bump_failures, forward_line, spawn_liveness_ticker};

// ---------------------------------------------------------------------------
// URL source entry point
// ---------------------------------------------------------------------------

pub async fn run(
    url: String,
    tx: mpsc::Sender<String>,
    liveness: Arc<Liveness>,
    conn: SourceConnConfig,
    cfg: ReconnectConfig,
    filter: LineFilter,
) -> Result<()> {
    let mut builder = reqwest::Client::builder()
        .danger_accept_invalid_certs(true); // allow vigild self-signed TLS
    if conn.connect_timeout_ms > 0 {
        builder = builder.connect_timeout(Duration::from_millis(conn.connect_timeout_ms));
    }
    if conn.read_timeout_ms > 0 {
        builder = builder.read_timeout(Duration::from_millis(conn.read_timeout_ms));
    }
    if conn.keepalive_interval_secs > 0 {
        builder = builder.tcp_keepalive(Duration::from_secs(conn.keepalive_interval_secs));
    }
    if let Some(proxy_url) = &conn.proxy_url {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy_url.as_str())
                .context("invalid --source-proxy URL")?,
        );
    }
    if conn.proxy_insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if let Some(ca_path) = &conn.proxy_cacert {
        for cert in load_pem_chain(ca_path)
            .with_context(|| format!("failed to load --source-proxy-cacert {}", ca_path.display()))?
        {
            builder = builder.add_root_certificate(cert);
        }
    }
    let client = builder.build()?;

    spawn_liveness_ticker(Arc::clone(&liveness));

    let mut backoff = Duration::from_millis(cfg.initial_delay_ms);
    let mut failures: u64 = 0;

    loop {
        liveness.tick();

        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!(url = %url, "connected");
                failures = 0;
                backoff = Duration::from_millis(cfg.initial_delay_ms);

                let byte_stream = resp
                    .bytes_stream()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
                let mut lines = tokio::io::BufReader::new(StreamReader::new(byte_stream)).lines();

                let mut clean_eof = true;
                loop {
                    let next = if conn.idle_timeout_ms > 0 {
                        match tokio::time::timeout(
                            Duration::from_millis(conn.idle_timeout_ms),
                            lines.next_line(),
                        ).await {
                            Ok(r) => r,
                            Err(_) => {
                                warn!(url = %url, "idle timeout — reconnecting");
                                clean_eof = false;
                                break;
                            }
                        }
                    } else {
                        lines.next_line().await
                    };
                    match next {
                        Ok(Some(line)) => forward_line(line, &tx, &filter),
                        Ok(None) => {
                            info!(url = %url, "stream EOF — reconnecting");
                            break;
                        }
                        Err(e) => {
                            warn!(url = %url, error = %e, "read error — reconnecting");
                            clean_eof = false;
                            break;
                        }
                    }
                }
                if !clean_eof {
                    failures = bump_failures(failures, cfg.max_retries, &url)?;
                }
            }
            Ok(resp) => {
                warn!(url = %url, status = %resp.status(), "unexpected HTTP status");
                failures = bump_failures(failures, cfg.max_retries, &url)?;
            }
            Err(e) => {
                warn!(url = %url, error = %e, backoff_ms = backoff.as_millis(), "connection failed");
                failures = bump_failures(failures, cfg.max_retries, &url)?;
            }
        }

        sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_millis(cfg.max_delay_ms));
    }
}

// ---------------------------------------------------------------------------
// PEM certificate chain loader for reqwest
// ---------------------------------------------------------------------------

/// Load all PEM-encoded certificates from `path`, supporting chain files with
/// multiple `-----BEGIN CERTIFICATE-----` blocks.
fn load_pem_chain(path: &Path) -> Result<Vec<reqwest::Certificate>> {
    let pem = std::fs::read_to_string(path)?;
    let mut certs = Vec::new();
    let mut block = String::new();
    for line in pem.lines() {
        block.push_str(line);
        block.push('\n');
        if line.trim() == "-----END CERTIFICATE-----" {
            certs.push(
                reqwest::Certificate::from_pem(block.as_bytes())
                    .context("invalid PEM certificate block")?,
            );
            block.clear();
        }
    }
    if certs.is_empty() {
        anyhow::bail!("no certificates found in {}", path.display());
    }
    Ok(certs)
}
