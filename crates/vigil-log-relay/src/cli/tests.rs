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
