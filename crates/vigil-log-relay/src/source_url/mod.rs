//! HTTP/HTTPS URL source — connects to a streaming endpoint via reqwest.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::TryStreamExt;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::io::StreamReader;
use tracing::{info, warn};
use vigil_files::{FILE_IO_TIMEOUT, PeriodicRead, ReadOutcome};
use vigil_tls::{ClientTls, PemFile};

use crate::source_http::{bump_failures, spawn_liveness_ticker, stream_loop};
use crate::{LineFilter, Liveness, ReconnectConfig, SourceConnConfig};

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
    // reqwest has no TLS settings of its own for the proxy: its CA and
    // `insecure` apply to the whole client.
    let insecure = conn.source_insecure || conn.proxy_insecure;
    let files = [
        conn.source_cacert.clone(),
        conn.proxy_cacert.clone(),
        conn.source_cert.clone(),
        conn.source_key.clone(),
    ];
    let mut builder = vigil_files::off_runtime("TLS files", move || {
        let [ca, proxy_ca, cert, key] = &files;
        ClientTls {
            insecure,
            ca: [
                ca.as_deref().map(|p| PemFile::new("--source-cacert", p)),
                proxy_ca
                    .as_deref()
                    .map(|p| PemFile::new("--source-proxy-cacert", p)),
            ]
            .into_iter()
            .flatten()
            .collect(),
            cert: cert.as_deref().map(|p| PemFile::new("--source-cert", p)),
            key: key.as_deref().map(|p| PemFile::new("--source-key", p)),
        }
        .apply(reqwest::Client::builder())
    })
    .await?;
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
        use vigil_types::no_proxy::{no_proxy_matches, parse_no_proxy};
        let no_proxy_entries = parse_no_proxy(conn.no_proxy.as_deref());
        let proxy_uri: reqwest::Url = proxy_url.parse().context("invalid --source-proxy URL")?;
        let proxy = reqwest::Proxy::custom(move |url| {
            let host = url.host_str().unwrap_or("");
            if no_proxy_matches(host, &no_proxy_entries) {
                None
            } else {
                Some(proxy_uri.clone())
            }
        });
        builder = builder.proxy(proxy);
    }
    let client = builder.build()?;

    spawn_liveness_ticker(Arc::clone(&liveness));

    let mut backoff = Duration::from_millis(cfg.initial_delay_ms);
    let mut failures: u64 = 0;
    // Read again per connection (rotated secrets); a dead NFS mount ties up
    // at most one blocking thread.
    let mut credentials = PeriodicRead::default();

    loop {
        liveness.tick();

        let mut request = client.get(&url);
        if let Some(path) = &conn.source_user_file {
            let p = path.clone();
            let read = match credentials.run(move || read_basic_credentials(&p)).await {
                ReadOutcome::Done(r) => r,
                ReadOutcome::TimedOut => Err(anyhow::anyhow!(
                    "{path:?}: not read within {}s (hanging file system, e.g. a dead NFS mount?)",
                    FILE_IO_TIMEOUT.as_secs()
                )),
                ReadOutcome::StillHanging => {
                    Err(anyhow::anyhow!("{path:?}: the previous read still hangs"))
                }
                ReadOutcome::Failed => Err(anyhow::anyhow!("{path:?}: the read panicked")),
            };
            match read {
                Ok((user, pass)) => request = request.basic_auth(user, Some(pass)),
                Err(e) => {
                    warn!(error = %format!("{e:#}"), "cannot read --source-user-file");
                    failures = bump_failures(failures, cfg.max_retries, &url)?;
                    sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_millis(cfg.max_delay_ms));
                    continue;
                }
            }
        }

        match request.send().await {
            Ok(resp) if resp.status().is_success() => {
                info!(url = %url, "connected");
                failures = 0;
                backoff = Duration::from_millis(cfg.initial_delay_ms);

                let byte_stream = resp.bytes_stream().map_err(std::io::Error::other);
                let lines = BufReader::new(StreamReader::new(byte_stream)).lines();

                let clean_eof = stream_loop(lines, &tx, &filter, conn.idle_timeout_ms, &url).await;
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
// Basic Auth credentials file
// ---------------------------------------------------------------------------

/// Read `user:password` from `path`. Only trailing newlines are stripped, so
/// passwords may contain spaces and further colons.
pub(crate) fn read_basic_credentials(path: &Path) -> Result<(String, String)> {
    let raw = vigil_files::read_text(path, 64 * 1024, vigil_files::PathRule::Any)
        .map_err(|e| anyhow::anyhow!("reading {e}"))?
        .with_context(|| format!("reading {path:?}: No such file or directory"))?;
    let line = raw.trim_end_matches(['\n', '\r']);
    let (user, pass) = line
        .split_once(':')
        .with_context(|| format!("{}: expected `user:password`", path.display()))?;
    anyhow::ensure!(!user.is_empty(), "{}: empty user name", path.display());
    Ok((user.to_owned(), pass.to_owned()))
}

#[cfg(test)]
mod tests;
