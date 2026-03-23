// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::time::Duration;

use axum::{Router, http::StatusCode, routing::get};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use indexmap::IndexMap;
use reqwest::Client as HttpClient;
use tower::Service;
use vigil_types::plan::HttpCheck;

use super::exec::probe_exec;
use super::http::probe_http;
use super::tcp::probe_tcp;

// ------------------------------------------------------------------
// Test server helpers
// ------------------------------------------------------------------

async fn spawn_http_server(app: Router) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

async fn spawn_tls_server(app: Router) -> (u16, Vec<u8>) {
    let (cert_ders, key_der) = crate::tls::generate_self_signed(&["localhost"]).unwrap();
    let cert_der = cert_ders[0].clone();
    let acceptor = crate::tls::acceptor_from_der(cert_ders, key_der).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            let Ok((stream, _peer)) = listener.accept().await else {
                break;
            };
            let acceptor = acceptor.clone();
            let app = app.clone();
            tokio::spawn(async move {
                match acceptor.accept(stream).await {
                    Err(_) => {}
                    Ok(tls_stream) => {
                        let io = TokioIo::new(tls_stream);
                        let svc =
                            hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
                                let mut r = app.clone();
                                async move { r.call(req.map(axum::body::Body::new)).await }
                            });
                        let _ = http1::Builder::new().serve_connection(io, svc).await;
                    }
                }
            });
        }
    });

    (port, cert_der)
}

use crate::testutil::init_crypto;

fn make_check(url: String) -> HttpCheck {
    HttpCheck {
        url,
        headers: IndexMap::new(),
        insecure: false,
        ca: None,
        success_statuses: vec![],
    }
}

fn shared_client() -> HttpClient {
    HttpClient::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

// ------------------------------------------------------------------
// Plain HTTP
// ------------------------------------------------------------------

#[tokio::test]
async fn http_2xx_passes() {
    let app = Router::new().route("/ok", get(|| async { StatusCode::OK }));
    let port = spawn_http_server(app).await;
    let check = make_check(format!("http://127.0.0.1:{port}/ok"));
    assert!(probe_http(&shared_client(), &check, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn http_4xx_fails() {
    let app = Router::new().route("/nf", get(|| async { StatusCode::NOT_FOUND }));
    let port = spawn_http_server(app).await;
    let check = make_check(format!("http://127.0.0.1:{port}/nf"));
    assert!(!probe_http(&shared_client(), &check, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn http_5xx_fails() {
    let app = Router::new().route("/err", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }));
    let port = spawn_http_server(app).await;
    let check = make_check(format!("http://127.0.0.1:{port}/err"));
    assert!(!probe_http(&shared_client(), &check, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn success_statuses_override_default() {
    let app = Router::new()
        .route("/redir", get(|| async { StatusCode::MOVED_PERMANENTLY }))
        .route("/nf", get(|| async { StatusCode::NOT_FOUND }));
    let port = spawn_http_server(app).await;

    // 301 passes when explicitly listed
    let mut check = make_check(format!("http://127.0.0.1:{port}/redir"));
    check.success_statuses = vec![301];
    assert!(probe_http(&shared_client(), &check, Duration::from_secs(5)).await);

    // 200 fails when not in the explicit list
    let mut check2 = make_check(format!("http://127.0.0.1:{port}/nf"));
    check2.success_statuses = vec![301];
    assert!(!probe_http(&shared_client(), &check2, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn custom_headers_are_forwarded() {
    let app = Router::new().route(
        "/hdr",
        get(|hdrs: axum::http::HeaderMap| async move {
            if hdrs.get("x-vigil-test").and_then(|v| v.to_str().ok()) == Some("yes") {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        }),
    );
    let port = spawn_http_server(app).await;
    let url = format!("http://127.0.0.1:{port}/hdr");

    let without = make_check(url.clone());
    assert!(!probe_http(&shared_client(), &without, Duration::from_secs(5)).await);

    let mut with_hdr = make_check(url);
    with_hdr
        .headers
        .insert("x-vigil-test".to_string(), "yes".to_string());
    assert!(probe_http(&shared_client(), &with_hdr, Duration::from_secs(5)).await);
}

// ------------------------------------------------------------------
// TLS
// ------------------------------------------------------------------

#[tokio::test]
async fn self_signed_rejected_by_default() {
    init_crypto();
    let app = Router::new().route("/", get(|| async { StatusCode::OK }));
    let (port, _cert_der) = spawn_tls_server(app).await;
    let check = make_check(format!("https://localhost:{port}/"));
    assert!(!probe_http(&shared_client(), &check, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn insecure_accepts_self_signed() {
    init_crypto();
    let app = Router::new().route("/", get(|| async { StatusCode::OK }));
    let (port, _cert_der) = spawn_tls_server(app).await;
    let mut check = make_check(format!("https://localhost:{port}/"));
    check.insecure = true;
    assert!(probe_http(&shared_client(), &check, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn ca_cert_accepts_self_signed() {
    init_crypto();
    let app = Router::new().route("/", get(|| async { StatusCode::OK }));
    let (port, cert_der) = spawn_tls_server(app).await;

    let ca_pem = crate::tls::cert_to_pem(&cert_der);
    let tmp_path = std::env::temp_dir().join(format!("vigil-test-ca-{port}.pem"));
    std::fs::write(&tmp_path, &ca_pem).unwrap();

    let mut check = make_check(format!("https://localhost:{port}/"));
    check.ca = Some(tmp_path.clone());
    let result = probe_http(&shared_client(), &check, Duration::from_secs(5)).await;
    let _ = std::fs::remove_file(&tmp_path);
    assert!(result);
}

#[tokio::test]
async fn ca_nonexistent_file_returns_false() {
    init_crypto();
    let mut check = make_check("https://localhost:1/".to_string());
    check.ca = Some(std::path::PathBuf::from("/nonexistent/ca.pem"));
    assert!(!probe_http(&shared_client(), &check, Duration::from_secs(1)).await);
}

// ------------------------------------------------------------------
// TCP check
// ------------------------------------------------------------------

#[tokio::test]
async fn tcp_check_success() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = listener.accept().await;
    });
    assert!(probe_tcp("127.0.0.1", port, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn tcp_check_connection_refused() {
    let port = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    assert!(!probe_tcp("127.0.0.1", port, Duration::from_millis(500)).await);
}

// ------------------------------------------------------------------
// Exec check
// ------------------------------------------------------------------

fn empty_svc_configs() -> IndexMap<String, vigil_types::plan::ServiceConfig> {
    IndexMap::new()
}

fn exec(command: &str) -> vigil_types::plan::ExecCheck {
    vigil_types::plan::ExecCheck {
        command: command.to_string(),
        service_context: None,
        environment: IndexMap::new(),
        user: None,
        user_id: None,
        group: None,
        group_id: None,
        working_dir: None,
    }
}

#[tokio::test]
async fn exec_check_true_succeeds() {
    let ok = probe_exec(&exec("true"), Duration::from_secs(5), &empty_svc_configs()).await;
    assert!(ok);
}

#[tokio::test]
async fn exec_check_false_fails() {
    let ok = probe_exec(&exec("false"), Duration::from_secs(5), &empty_svc_configs()).await;
    assert!(!ok);
}

#[tokio::test]
async fn exec_check_timeout_returns_false() {
    let ok = probe_exec(
        &exec("sleep 30"),
        Duration::from_millis(100),
        &empty_svc_configs(),
    )
    .await;
    assert!(!ok);
}

#[tokio::test]
async fn exec_check_env_var_inherited() {
    let mut e = exec("test \"$VIGIL_TEST_VAR\" = hello");
    e.environment
        .insert("VIGIL_TEST_VAR".to_string(), "hello".to_string());
    let ok = probe_exec(&e, Duration::from_secs(5), &empty_svc_configs()).await;
    assert!(ok);
}

#[tokio::test]
async fn exec_check_working_dir() {
    let mut e = exec("test -d .");
    e.working_dir = Some("/tmp".to_string());
    let ok = probe_exec(&e, Duration::from_secs(5), &empty_svc_configs()).await;
    assert!(ok);
}

#[tokio::test]
async fn exec_check_invalid_working_dir_fails() {
    let mut e = exec("true");
    e.working_dir = Some("/nonexistent_dir_xyz_abc".to_string());
    let ok = probe_exec(&e, Duration::from_secs(5), &empty_svc_configs()).await;
    assert!(!ok);
}

#[tokio::test]
async fn exec_nonexistent_command_returns_false() {
    // A command that cannot be spawned (the shell will exit non-zero for a
    // missing binary invoked via "sh -c", but the outer sh exits with 127).
    // We test an absolute path that does not exist so sh exits non-zero.
    let ok = probe_exec(
        &exec("/nonexistent/binary_xyz_abc --flag"),
        Duration::from_secs(5),
        &empty_svc_configs(),
    )
    .await;
    assert!(!ok);
}

#[tokio::test]
async fn exec_inherits_service_env() {
    // Build a ServiceConfig with an environment variable.
    let mut svc = vigil_types::plan::ServiceConfig::default();
    svc.environment
        .insert("VIGIL_SVC_VAR".to_string(), "from_service".to_string());

    let mut svcs: IndexMap<String, vigil_types::plan::ServiceConfig> = IndexMap::new();
    svcs.insert("my-svc".to_string(), svc);

    // Exec check references the service and tests that the variable is set.
    let mut e = exec("test \"$VIGIL_SVC_VAR\" = from_service");
    e.service_context = Some("my-svc".to_string());

    let ok = probe_exec(&e, Duration::from_secs(5), &svcs).await;
    assert!(ok);
}

#[tokio::test]
async fn exec_check_env_overrides_service_env() {
    // Service sets FOO=service; check overrides FOO=check.
    let mut svc = vigil_types::plan::ServiceConfig::default();
    svc.environment
        .insert("FOO".to_string(), "service".to_string());

    let mut svcs: IndexMap<String, vigil_types::plan::ServiceConfig> = IndexMap::new();
    svcs.insert("my-svc".to_string(), svc);

    let mut e = exec("test \"$FOO\" = check");
    e.service_context = Some("my-svc".to_string());
    e.environment.insert("FOO".to_string(), "check".to_string());

    let ok = probe_exec(&e, Duration::from_secs(5), &svcs).await;
    assert!(ok);
}

#[tokio::test]
async fn exec_nonexistent_service_context_falls_back_gracefully() {
    // service_context points to a service that is not in the map; exec should
    // still work (no env inheritance, no crash).
    let mut e = exec("true");
    e.service_context = Some("no-such-service".to_string());

    let ok = probe_exec(&e, Duration::from_secs(5), &empty_svc_configs()).await;
    assert!(ok);
}

// ------------------------------------------------------------------
// perform() dispatcher
// ------------------------------------------------------------------

use super::perform;
use vigil_types::plan::{CheckConfig, TcpCheck};

fn make_http_client() -> HttpClient {
    HttpClient::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test]
async fn no_check_configured_returns_true() {
    // When all of http/tcp/exec are None the dispatcher should return true.
    let config = CheckConfig::default();
    let result = perform(
        &config,
        Duration::from_secs(5),
        &make_http_client(),
        &empty_svc_configs(),
    )
    .await;
    assert!(result);
}

#[tokio::test]
async fn tcp_check_via_perform_dispatcher() {
    // Route a TCP check through perform() to exercise the dispatcher branch.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = listener.accept().await;
    });

    let config = CheckConfig {
        tcp: Some(TcpCheck {
            host: Some("127.0.0.1".to_string()),
            port,
        }),
        ..Default::default()
    };
    let result = perform(
        &config,
        Duration::from_secs(5),
        &make_http_client(),
        &empty_svc_configs(),
    )
    .await;
    assert!(result);
}

#[tokio::test]
async fn tcp_check_default_host_via_perform() {
    // TcpCheck with host: None → perform() falls back to "localhost".
    // Bind on localhost so the connection succeeds.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = listener.accept().await;
    });

    let config = CheckConfig {
        tcp: Some(TcpCheck { host: None, port }),
        ..Default::default()
    };
    let result = perform(
        &config,
        Duration::from_secs(5),
        &make_http_client(),
        &empty_svc_configs(),
    )
    .await;
    assert!(result);
}
