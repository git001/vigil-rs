use super::Cli;
use clap::Parser;

fn parse(args: &[&str]) -> Result<Cli, String> {
    let mut argv = vec!["vigil-log-relay", "--stdout-sink"];
    argv.extend_from_slice(args);
    let cli = Cli::try_parse_from(argv).map_err(|e| e.to_string())?;
    cli.validate()?;
    Ok(cli)
}

#[test]
fn url_only_options_rejected_with_kubernetes() {
    for extra in [
        &["--source-cert", "c", "--source-key", "k"][..],
        &["--source-user-file", "f"][..],
        &["--source-cacert", "ca"][..],
        &["--source-insecure"][..],
    ] {
        let mut args = vec!["--kubernetes"];
        args.extend_from_slice(extra);
        assert!(
            parse(&args).is_err(),
            "{extra:?} accepted with --kubernetes"
        );
    }
}

#[test]
fn other_sources_rejected_with_kubernetes() {
    let err = parse(&["--kubernetes", "--source-url", "https://x/"]).unwrap_err();
    assert!(
        err.contains("--source-url cannot be used with --kubernetes"),
        "{err}"
    );
    let err = parse(&["--kubernetes", "--source-socket", "/s"]).unwrap_err();
    assert!(
        err.contains("--source-socket cannot be used with --kubernetes"),
        "{err}"
    );
}

#[test]
fn url_only_options_rejected_with_socket() {
    assert!(parse(&["--source-socket", "/s", "--source-user-file", "f"]).is_err());
    assert!(parse(&["--source-socket", "/s", "--source-insecure"]).is_err());
}

#[test]
fn url_only_options_accepted_with_url() {
    let cli = parse(&[
        "--source-url",
        "https://x/",
        "--source-cert",
        "c",
        "--source-key",
        "k",
        "--source-user-file",
        "f",
        "--source-cacert",
        "ca",
        "--source-insecure",
    ])
    .unwrap();
    assert!(cli.conn.source_insecure);
}

#[test]
fn cert_and_key_require_each_other() {
    assert!(parse(&["--source-url", "https://x/", "--source-cert", "c"]).is_err());
    assert!(parse(&["--source-url", "https://x/", "--source-key", "k"]).is_err());
}

#[test]
fn proxy_options_still_allowed_with_kubernetes() {
    parse(&["--kubernetes", "--source-proxy", "http://p:3128"]).unwrap();
}

#[test]
fn a_renamed_environment_variable_is_an_error() {
    assert_eq!(
        super::renamed_env(|name| name == "DEST_READ_TIMEOUT"),
        Err("DEST_READ_TIMEOUT was renamed to DEST_WRITE_TIMEOUT".to_string())
    );
    assert_eq!(super::renamed_env(|_| false), Ok(()));
}

#[test]
fn dest_write_timeout_replaces_dest_read_timeout() {
    let cli = parse(&["--source-url", "http://x/", "--dest-write-timeout", "500"]).unwrap();
    assert_eq!(cli.sink.dest_write_timeout, 500);
    assert!(parse(&["--source-url", "http://x/", "--dest-read-timeout", "500"]).is_err());
}

#[test]
fn send_buffer_lines_default_and_range() {
    assert_eq!(
        parse(&["--source-url", "http://x/"])
            .unwrap()
            .sink
            .send_buffer_lines,
        8192
    );
    let cli = parse(&["--source-url", "http://x/", "--send-buffer-lines", "64"]).unwrap();
    assert_eq!(cli.sink.send_buffer_lines, 64);
    for bad in ["63", "1048577", "lots"] {
        assert!(parse(&["--source-url", "http://x/", "--send-buffer-lines", bad]).is_err());
    }
}
