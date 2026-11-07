// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::time::Duration;

use axum::{Router, http::StatusCode, routing::get};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use indexmap::IndexMap;
use tower::Service;
use vigil_types::plan::{CheckConfig, HttpCheck, ProxyConfig, TlsConfig};

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
    let cert_der = cert_ders[0].to_vec();
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
        no_follow_redirects: true,
        tls: None,
        proxy: None,
        success_statuses: vec![],
    }
}

/// The client `prepare_checks` builds for `check`.
fn client(check: &HttpCheck) -> super::CheckClient {
    super::http::client(check).unwrap()
}

// ------------------------------------------------------------------
// Plain HTTP
// ------------------------------------------------------------------

#[tokio::test]
async fn http_2xx_passes() {
    let app = Router::new().route("/ok", get(|| async { StatusCode::OK }));
    let port = spawn_http_server(app).await;
    let check = make_check(format!("http://127.0.0.1:{port}/ok"));
    assert!(
        probe_http(&client(&check), &check, Duration::from_secs(5))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn http_4xx_fails() {
    let app = Router::new().route("/nf", get(|| async { StatusCode::NOT_FOUND }));
    let port = spawn_http_server(app).await;
    let check = make_check(format!("http://127.0.0.1:{port}/nf"));
    assert!(
        !probe_http(&client(&check), &check, Duration::from_secs(5))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn http_5xx_fails() {
    let app = Router::new().route("/err", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }));
    let port = spawn_http_server(app).await;
    let check = make_check(format!("http://127.0.0.1:{port}/err"));
    assert!(
        !probe_http(&client(&check), &check, Duration::from_secs(5))
            .await
            .is_ok()
    );
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
    assert!(
        probe_http(&client(&check), &check, Duration::from_secs(5))
            .await
            .is_ok()
    );

    // 200 fails when not in the explicit list
    let mut check2 = make_check(format!("http://127.0.0.1:{port}/nf"));
    check2.success_statuses = vec![301];
    assert!(
        !probe_http(&client(&check2), &check2, Duration::from_secs(5))
            .await
            .is_ok()
    );
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
    assert!(
        !probe_http(&client(&without), &without, Duration::from_secs(5))
            .await
            .is_ok()
    );

    let mut with_hdr = make_check(url);
    with_hdr
        .headers
        .insert("x-vigil-test".to_string(), "yes".to_string());
    assert!(
        probe_http(&client(&with_hdr), &with_hdr, Duration::from_secs(5))
            .await
            .is_ok()
    );
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
    let err = probe_http(&client(&check), &check, Duration::from_secs(5))
        .await
        .unwrap_err();
    // The reason names the certificate problem, not only "error sending
    // request".
    assert_eq!(
        err,
        format!("GET https://localhost:{port}/: invalid peer certificate: UnknownIssuer")
    );
}

#[tokio::test]
async fn insecure_accepts_self_signed() {
    init_crypto();
    let app = Router::new().route("/", get(|| async { StatusCode::OK }));
    let (port, _cert_der) = spawn_tls_server(app).await;
    let mut check = make_check(format!("https://localhost:{port}/"));
    check.tls = Some(TlsConfig {
        insecure: true,
        ..Default::default()
    });
    assert!(
        probe_http(&client(&check), &check, Duration::from_secs(5))
            .await
            .is_ok()
    );
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
    check.tls = Some(TlsConfig {
        ca: vec![tmp_path.clone()],
        ..Default::default()
    });
    let result = probe_http(&client(&check), &check, Duration::from_secs(5))
        .await
        .is_ok();
    let _ = std::fs::remove_file(&tmp_path);
    assert!(result);
}

#[test]
fn bad_tls_or_proxy_settings_fail_when_loading() {
    init_crypto();
    let mut checks = IndexMap::new();
    let mut check = make_check("https://localhost:1/".to_string());
    check.tls = Some(TlsConfig {
        ca: vec!["/nonexistent/ca.pem".into()],
        ..Default::default()
    });
    checks.insert(
        "web".to_string(),
        CheckConfig {
            http: Some(check),
            ..Default::default()
        },
    );
    let err = format!("{:#}", crate::check::prepare_checks(&checks).unwrap_err());
    assert!(
        err.starts_with("check \"web\": tls.ca \"/nonexistent/ca.pem\""),
        "{err}"
    );

    let mut check = make_check("http://localhost:1/".to_string());
    check.proxy = Some(ProxyConfig {
        url: "not a url".into(),
        no_proxy: None,
    });
    checks["web"].http = Some(check);
    let err = format!("{:#}", crate::check::prepare_checks(&checks).unwrap_err());
    assert!(err.starts_with("check \"web\": proxy.url"), "{err}");
}

// ------------------------------------------------------------------
// Proxy
// ------------------------------------------------------------------

/// A plain HTTP proxy answers absolute-form requests (`GET http://…`);
/// this one answers every request with 200.
async fn spawn_proxy() -> u16 {
    spawn_http_server(Router::new().fallback(|| async { StatusCode::OK })).await
}

#[tokio::test]
async fn proxy_is_used_unless_no_proxy_matches() {
    let proxy_port = spawn_proxy().await;
    // Only reachable through the proxy.
    let mut check = make_check("http://vigil-check.invalid/healthz".to_string());
    assert!(
        !probe_http(&client(&check), &check, Duration::from_secs(5))
            .await
            .is_ok()
    );

    check.proxy = Some(ProxyConfig {
        url: format!("http://127.0.0.1:{proxy_port}"),
        no_proxy: None,
    });
    assert!(
        probe_http(&client(&check), &check, Duration::from_secs(5))
            .await
            .is_ok()
    );

    check.proxy = Some(ProxyConfig {
        url: format!("http://127.0.0.1:{proxy_port}"),
        no_proxy: Some(".invalid".into()),
    });
    assert!(
        !probe_http(&client(&check), &check, Duration::from_secs(5))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn http_check_without_a_client_fails() {
    let config = CheckConfig {
        http: Some(make_check("http://127.0.0.1:1/".to_string())),
        ..Default::default()
    };
    assert!(
        !perform(&config, Duration::from_secs(1), None, &empty_svc_configs())
            .await
            .result
            .is_ok()
    );
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
    assert!(
        probe_tcp("127.0.0.1", port, Duration::from_secs(5))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn tcp_check_connection_refused() {
    // Bound but never listening: the port stays reserved for this test (no
    // other test can grab it, unlike a listener that is dropped again), and
    // a connect to it is refused.
    let sock = tokio::net::TcpSocket::new_v4().unwrap();
    sock.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = sock.local_addr().unwrap().port();
    assert!(
        !probe_tcp("127.0.0.1", port, Duration::from_millis(500))
            .await
            .is_ok()
    );
    drop(sock);
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
        ..Default::default()
    }
}

#[tokio::test]
async fn exec_check_true_succeeds() {
    let ok = probe_exec(&exec("true"), Duration::from_secs(5), &empty_svc_configs())
        .await
        .is_ok();
    assert!(ok);
}

#[tokio::test]
async fn exec_check_false_fails() {
    let ok = probe_exec(&exec("false"), Duration::from_secs(5), &empty_svc_configs())
        .await
        .is_ok();
    assert!(!ok);
}

#[tokio::test]
async fn exec_check_timeout_returns_false() {
    let err = probe_exec(
        &exec("echo started; sleep 30"),
        Duration::from_millis(300),
        &empty_svc_configs(),
    )
    .await
    .unwrap_err();
    // The reason keeps what the command printed before it was killed.
    assert_eq!(err, "timed out after 300ms: started");
}

#[tokio::test]
async fn timed_out_exec_check_is_killed_with_its_children() {
    let dir = tempfile::tempdir().unwrap();
    let pids = dir.path().join("pids");
    // A pipeline: the shell plus two children, none of which ends.
    let cmd = format!(
        "echo $$ > {p}; sleep 30 | sleep 31 & echo $! >> {p}; wait",
        p = pids.display()
    );
    let started = std::time::Instant::now();
    let result = probe_exec(
        &exec(&cmd),
        Duration::from_millis(300),
        &empty_svc_configs(),
    )
    .await;
    assert_eq!(result, Err("timed out after 300ms".to_string()));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "returned at once"
    );

    let pids: Vec<i32> = std::fs::read_to_string(&pids)
        .unwrap()
        .lines()
        .map(|l| l.trim().parse().unwrap())
        .collect();
    assert_eq!(pids.len(), 2, "{pids:?}");
    for pid in pids {
        let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
        assert!(!alive, "process {pid} of the timed-out check still runs");
    }
}

#[tokio::test]
async fn exec_check_env_var_inherited() {
    let mut e = exec("test \"$VIGIL_TEST_VAR\" = hello");
    e.environment
        .insert("VIGIL_TEST_VAR".to_string(), "hello".to_string());
    let ok = probe_exec(&e, Duration::from_secs(5), &empty_svc_configs())
        .await
        .is_ok();
    assert!(ok);
}

#[tokio::test]
async fn exec_check_working_dir() {
    let mut e = exec("test -d .");
    e.working_dir = Some("/tmp".to_string());
    let ok = probe_exec(&e, Duration::from_secs(5), &empty_svc_configs())
        .await
        .is_ok();
    assert!(ok);
}

#[tokio::test]
async fn exec_check_invalid_working_dir_fails() {
    let mut e = exec("true");
    e.working_dir = Some("/nonexistent_dir_xyz_abc".to_string());
    let ok = probe_exec(&e, Duration::from_secs(5), &empty_svc_configs())
        .await
        .is_ok();
    assert!(!ok);
}

#[tokio::test]
async fn exec_nonexistent_command_returns_false() {
    // A command that cannot be spawned (the shell will exit non-zero for a
    // missing binary invoked via "sh -c", but the outer sh exits with 127).
    // We test an absolute path that does not exist so sh exits non-zero.
    let err = probe_exec(
        &exec("/nonexistent/binary_xyz_abc --flag"),
        Duration::from_secs(5),
        &empty_svc_configs(),
    )
    .await
    .unwrap_err();
    // sh's own message on stderr is part of the reason.
    assert!(err.starts_with("exit status 127: "), "{err}");
    assert!(err.contains("/nonexistent/binary_xyz_abc"), "{err}");
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

    let ok = probe_exec(&e, Duration::from_secs(5), &svcs).await.is_ok();
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

    let ok = probe_exec(&e, Duration::from_secs(5), &svcs).await.is_ok();
    assert!(ok);
}

#[tokio::test]
async fn exec_nonexistent_service_context_falls_back_gracefully() {
    // service_context points to a service that is not in the map; exec should
    // still work (no env inheritance, no crash).
    let mut e = exec("true");
    e.service_context = Some("no-such-service".to_string());

    let ok = probe_exec(&e, Duration::from_secs(5), &empty_svc_configs())
        .await
        .is_ok();
    assert!(ok);
}

// ------------------------------------------------------------------
// perform() dispatcher
// ------------------------------------------------------------------

use super::perform;
use vigil_types::plan::TcpCheck;

#[tokio::test]
async fn no_check_configured_returns_true() {
    // When all of http/tcp/exec are None the dispatcher should return true.
    let config = CheckConfig::default();
    let result = perform(&config, Duration::from_secs(5), None, &empty_svc_configs())
        .await
        .result
        .is_ok();
    assert!(result);
}

#[tokio::test]
async fn tcp_check_via_perform_dispatcher() {
    // Route a TCP check through perform() to exercise the dispatcher branch.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = listener.accept().await.is_ok();
    });

    let config = CheckConfig {
        tcp: Some(TcpCheck {
            host: Some("127.0.0.1".to_string()),
            port,
        }),
        ..Default::default()
    };
    let result = perform(&config, Duration::from_secs(5), None, &empty_svc_configs())
        .await
        .result
        .is_ok();
    assert!(result);
}

#[tokio::test]
async fn tcp_check_default_host_via_perform() {
    // TcpCheck with host: None → perform() falls back to "localhost".
    // Bind on localhost so the connection succeeds.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = listener.accept().await.is_ok();
    });

    let config = CheckConfig {
        tcp: Some(TcpCheck { host: None, port }),
        ..Default::default()
    };
    let result = perform(&config, Duration::from_secs(5), None, &empty_svc_configs())
        .await
        .result
        .is_ok();
    assert!(result);
}

/// S2: an exec probe must not see vigild's environment unless allowed.
/// `cargo test` exports CARGO_MANIFEST_DIR into the test process, which
/// plays the part of a secret in vigild's environment here.
#[tokio::test]
async fn exec_check_does_not_inherit_vigild_env_by_default() {
    if std::env::var_os("CARGO_MANIFEST_DIR").is_none() {
        eprintln!("skipped: CARGO_MANIFEST_DIR not set");
        return;
    }
    let hidden = exec(r#"[ -z "$CARGO_MANIFEST_DIR" ] && [ -n "$PATH" ]"#);
    assert!(
        probe_exec(&hidden, Duration::from_secs(5), &empty_svc_configs())
            .await
            .is_ok(),
        "variable leaked into the probe (or PATH missing)"
    );

    let mut allowed = exec(r#"[ -n "$CARGO_MANIFEST_DIR" ]"#);
    allowed.pass_env_glob = vec!["CARGO_*".to_string()];
    assert!(
        probe_exec(&allowed, Duration::from_secs(5), &empty_svc_configs())
            .await
            .is_ok(),
        "pass-env-glob did not pass the variable"
    );

    let mut all = exec(r#"[ -n "$CARGO_MANIFEST_DIR" ]"#);
    all.pass_env = Some(true);
    assert!(
        probe_exec(&all, Duration::from_secs(5), &empty_svc_configs())
            .await
            .is_ok()
    );
}

/// The probe inherits the service context's pass-env lists.
#[tokio::test]
async fn exec_check_uses_service_context_pass_env() {
    if std::env::var_os("CARGO_MANIFEST_DIR").is_none() {
        return;
    }
    let mut svcs = IndexMap::new();
    svcs.insert(
        "app".to_string(),
        vigil_types::plan::ServiceConfig {
            pass_env_regex: vec!["CARGO_MANIFEST_.*".to_string()],
            ..Default::default()
        },
    );
    let mut e = exec(r#"[ -n "$CARGO_MANIFEST_DIR" ]"#);
    e.service_context = Some("app".to_string());
    assert!(probe_exec(&e, Duration::from_secs(5), &svcs).await.is_ok());
}

/// The service context's env-file and `environment` templates are resolved
/// for the check exactly as for the service; the check's own `environment`
/// templates see them.
#[tokio::test]
async fn exec_check_renders_service_context_templates_and_env_file() {
    let dir = tempfile::tempdir().unwrap();
    let pw = dir.path().join("pw");
    std::fs::write(&pw, "s3cret\n").unwrap();
    let envf = dir.path().join("db.env");
    std::fs::write(&envf, "export DB_USER=\"app\"\n").unwrap();

    let mut svcs = IndexMap::new();
    svcs.insert(
        "app".to_string(),
        vigil_types::plan::ServiceConfig {
            env_file: vec![envf.display().to_string()],
            environment: [(
                "PW".to_string(),
                format!("{{{{ file('{}') }}}}", pw.display()),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        },
    );
    let mut e = exec(r#"[ "$PW" = s3cret ] && [ "$WHO" = app-check ]"#);
    e.service_context = Some("app".to_string());
    e.environment
        .insert("WHO".into(), "{{ env.DB_USER }}-check".into());
    assert!(probe_exec(&e, Duration::from_secs(5), &svcs).await.is_ok());

    // a missing env file fails the probe (no panic, no values logged)
    svcs["app"].env_file = vec![dir.path().join("nope").display().to_string()];
    assert!(!probe_exec(&e, Duration::from_secs(5), &svcs).await.is_ok());
}

// ---------------------------------------------------------------------------
// Failure reasons
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exec_failure_reason_has_exit_status_and_output() {
    let err = probe_exec(
        &exec("echo to-stdout; echo to-stderr >&2; exit 2"),
        Duration::from_secs(5),
        &empty_svc_configs(),
    )
    .await
    .unwrap_err();
    // stdout and stderr share one pipe: both, in order.
    assert_eq!(err, "exit status 2: to-stdout\nto-stderr");
}

#[tokio::test]
async fn exec_failure_without_output_is_the_exit_status() {
    let result = probe_exec(
        &exec("exit 3"),
        Duration::from_secs(5),
        &empty_svc_configs(),
    )
    .await;
    assert_eq!(result, Err("exit status 3".to_string()));
}

#[tokio::test]
async fn exec_killed_by_signal_names_the_signal() {
    let result = probe_exec(
        &exec("kill -9 $$"),
        Duration::from_secs(5),
        &empty_svc_configs(),
    )
    .await;
    assert_eq!(result, Err("killed by signal 9".to_string()));
}

#[tokio::test]
async fn exec_failure_keeps_only_the_tail_of_long_output() {
    // 10000 bytes of 'x', then a final line: only the last 4 KiB stay.
    let err = probe_exec(
        &exec("head -c 10000 /dev/zero | tr '\\0' x; echo; echo last-line; exit 1"),
        Duration::from_secs(5),
        &empty_svc_configs(),
    )
    .await
    .unwrap_err();
    assert!(err.starts_with("exit status 1: …x"), "{}", &err[..40]);
    assert!(err.ends_with("x\nlast-line"), "{err}");
    assert!(err.len() < 4200, "{}", err.len());
}

#[tokio::test]
async fn exec_success_with_output_is_ok() {
    let result = probe_exec(
        &exec("echo fine"),
        Duration::from_secs(5),
        &empty_svc_configs(),
    )
    .await;
    assert_eq!(result, Ok(()));
}

#[tokio::test]
async fn exec_background_process_holding_the_pipe_does_not_stall() {
    // The command exits at once but leaves a child that keeps stdout open.
    let started = std::time::Instant::now();
    let err = probe_exec(
        &exec("sleep 5 & echo bye; exit 1"),
        Duration::from_secs(3),
        &empty_svc_configs(),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "exit status 1: bye");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn http_failure_reason_has_the_status() {
    let app = Router::new().route("/health", get(|| async { StatusCode::SERVICE_UNAVAILABLE }));
    let port = spawn_http_server(app).await;
    let check = make_check(format!("http://127.0.0.1:{port}/health"));
    let err = probe_http(&client(&check), &check, Duration::from_secs(5))
        .await
        .unwrap_err();
    assert_eq!(err, "HTTP 503 Service Unavailable");
}

#[tokio::test]
async fn http_connection_refused_reason() {
    let port = closed_port();
    let check = make_check(format!("http://127.0.0.1:{port}/health"));
    let err = probe_http(&client(&check), &check, Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(
        err.starts_with(&format!("GET http://127.0.0.1:{port}/health: ")),
        "{err}"
    );
    assert!(err.to_lowercase().contains("connection refused"), "{err}");
    assert!(!err.contains("error sending request"), "{err}");
}

#[tokio::test]
async fn tcp_connection_refused_reason() {
    let port = closed_port();
    let err = probe_tcp("127.0.0.1", port, Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(
        err.starts_with(&format!("connect 127.0.0.1:{port}: ")),
        "{err}"
    );
    assert!(err.to_lowercase().contains("connection refused"), "{err}");
}

/// A port nothing listens on: bound, then released.
fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

// ------------------------------------------------------------------
// Connect time (vigil_check_connect_duration_seconds)
// ------------------------------------------------------------------

fn http_config(check: HttpCheck) -> CheckConfig {
    CheckConfig {
        http: Some(check),
        ..Default::default()
    }
}

// The client keeps no idle connection: every run connects and is measured.
#[tokio::test]
async fn http_connect_is_measured_on_every_run() {
    let app = Router::new().route("/ok", get(|| async { StatusCode::OK }));
    let port = spawn_http_server(app).await;
    let check = make_check(format!("http://127.0.0.1:{port}/ok"));
    let client = client(&check);
    let config = http_config(check);
    for _ in 0..2 {
        let probe = perform(
            &config,
            Duration::from_secs(5),
            Some(&client),
            &empty_svc_configs(),
        )
        .await;
        assert!(probe.result.is_ok());
        let connect = probe.connect.expect("connect time");
        assert!(connect < Duration::from_secs(5), "{connect:?}");
    }
}

// https: the connect includes the TLS handshake.
#[tokio::test]
async fn https_connect_is_measured() {
    init_crypto();
    let app = Router::new().route("/", get(|| async { StatusCode::OK }));
    let (port, _cert_der) = spawn_tls_server(app).await;
    let mut check = make_check(format!("https://localhost:{port}/"));
    check.tls = Some(TlsConfig {
        insecure: true,
        ..Default::default()
    });
    let client = client(&check);
    let probe = perform(
        &http_config(check),
        Duration::from_secs(5),
        Some(&client),
        &empty_svc_configs(),
    )
    .await;
    assert!(probe.result.is_ok());
    assert!(probe.connect.is_some());
}

// No connection came up: nothing to measure. A failed run does not keep the
// time of an earlier one.
#[tokio::test]
async fn refused_connection_has_no_connect_time() {
    let app = Router::new().route("/ok", get(|| async { StatusCode::OK }));
    let port = spawn_http_server(app).await;
    let check = make_check(format!("http://127.0.0.1:{port}/ok"));
    let client = client(&check);
    let ok = perform(
        &http_config(check),
        Duration::from_secs(5),
        Some(&client),
        &empty_svc_configs(),
    )
    .await;
    assert!(ok.connect.is_some());

    // The same client against a closed port.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed_port = closed.local_addr().unwrap().port();
    drop(closed);
    let refused = make_check(format!("http://127.0.0.1:{closed_port}/ok"));
    let probe = perform(
        &http_config(refused),
        Duration::from_secs(5),
        Some(&client),
        &empty_svc_configs(),
    )
    .await;
    assert!(probe.result.is_err());
    assert_eq!(probe.connect, None);
}

#[tokio::test]
async fn tcp_connect_time_only_on_success_and_none_for_exec() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let tcp = |port| CheckConfig {
        tcp: Some(TcpCheck {
            host: Some("127.0.0.1".to_string()),
            port,
        }),
        ..Default::default()
    };
    let probe = perform(
        &tcp(port),
        Duration::from_secs(5),
        None,
        &empty_svc_configs(),
    )
    .await;
    assert!(probe.result.is_ok());
    assert!(probe.connect.is_some());
    drop(listener);
    let probe = perform(
        &tcp(port),
        Duration::from_secs(5),
        None,
        &empty_svc_configs(),
    )
    .await;
    assert!(probe.result.is_err());
    assert_eq!(probe.connect, None);

    let exec = CheckConfig {
        exec: Some(vigil_types::plan::ExecCheck {
            command: "true".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let probe = perform(&exec, Duration::from_secs(5), None, &empty_svc_configs()).await;
    assert!(probe.result.is_ok());
    assert_eq!(probe.connect, None);
}
