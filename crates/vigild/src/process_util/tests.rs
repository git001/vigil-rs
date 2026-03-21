// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use super::*;
use crate::process_util::parse::shell_words;

#[test]
fn parse_simple() {
    assert_eq!(
        parse_command("/bin/foo bar baz").unwrap(),
        vec!["/bin/foo", "bar", "baz"]
    );
}

#[test]
fn parse_default_args() {
    assert_eq!(
        parse_command("/bin/foo arg1 [ --port 8080 ]").unwrap(),
        vec!["/bin/foo", "arg1", "--port", "8080"]
    );
}

#[test]
fn parse_quoted() {
    assert_eq!(
        parse_command(r#"/bin/foo "hello world""#).unwrap(),
        vec!["/bin/foo", "hello world"]
    );
}

#[test]
fn parse_empty_defaults() {
    assert_eq!(parse_command("/bin/foo [ ]").unwrap(), vec!["/bin/foo"]);
}

#[test]
fn parse_shell_if_bracket_in_quoted_arg() {
    // `[ ... ]` inside a single-quoted sh -c argument must NOT be treated
    // as the default-args bracket syntax.
    let cmd = r#"sh -c 'if [ $((i % 7)) -eq 0 ]; then echo err; fi'"#;
    let argv = parse_command(cmd).unwrap();
    assert_eq!(argv[0], "sh");
    assert_eq!(argv[1], "-c");
    assert!(
        argv[2].contains("if ["),
        "shell script preserved: {:?}",
        argv[2]
    );
    assert_eq!(argv.len(), 3);
}

#[test]
fn parse_newlines_as_whitespace() {
    // Newlines outside quotes (e.g. from YAML folded scalars) are treated
    // as word separators, not embedded in tokens.
    let cmd = "sh -c 'script'\n";
    assert_eq!(parse_command(cmd).unwrap(), vec!["sh", "-c", "script"]);
}

#[test]
fn parse_command_unmatched_open_bracket_errors() {
    let err = parse_command("foo [ bar").unwrap_err();
    assert!(err.to_string().contains("unmatched"));
}

#[test]
fn parse_command_malformed_bracket_order_errors() {
    // Close ']' appears before the open '[ ' — malformed.
    let err = parse_command("foo ] bar [ baz").unwrap_err();
    assert!(err.to_string().contains("malformed"));
}

// shell_words edge cases
#[test]
fn shell_words_unterminated_single_quote_errors() {
    let err = shell_words("foo 'bar").unwrap_err();
    assert!(err.to_string().contains("unterminated"));
}

#[test]
fn shell_words_unterminated_double_quote_errors() {
    let err = shell_words("foo \"bar").unwrap_err();
    assert!(err.to_string().contains("unterminated"));
}

#[test]
fn shell_words_tab_and_newline_as_separator() {
    assert_eq!(shell_words("a\tb\nc").unwrap(), vec!["a", "b", "c"]);
}

#[test]
fn shell_words_backslash_escape_preserves_space() {
    assert_eq!(shell_words("foo\\ bar").unwrap(), vec!["foo bar"]);
}

#[test]
fn shell_words_double_quoted_space() {
    assert_eq!(shell_words("\"foo bar\"").unwrap(), vec!["foo bar"]);
}

#[test]
fn shell_words_single_quoted_space() {
    assert_eq!(shell_words("'foo bar'").unwrap(), vec!["foo bar"]);
}

// resolve_uid
#[test]
fn resolve_uid_none_none_returns_none() {
    assert!(resolve_uid(None, None).unwrap().is_none());
}

#[test]
fn resolve_uid_id_only_returns_uid() {
    let uid = resolve_uid(None, Some(0)).unwrap();
    assert_eq!(uid, Some(nix::unistd::Uid::from_raw(0)));
}

#[test]
fn resolve_uid_unknown_name_errors() {
    let err = resolve_uid(Some("_no_such_user_xyz_"), None).unwrap_err();
    assert!(err.to_string().contains("not found") || err.to_string().contains("getpwnam"));
}

#[test]
fn resolve_uid_name_and_matching_id() {
    // "root" always has uid 0 on Linux.
    let uid = resolve_uid(Some("root"), Some(0)).unwrap();
    assert_eq!(uid, Some(nix::unistd::Uid::from_raw(0)));
}

#[test]
fn resolve_uid_name_and_mismatched_id_errors() {
    let err = resolve_uid(Some("root"), Some(9999)).unwrap_err();
    assert!(err.to_string().contains("uid") || err.to_string().contains("user-id"));
}

// resolve_gid
#[test]
fn resolve_gid_none_none_returns_none() {
    assert!(resolve_gid(None, None).unwrap().is_none());
}

#[test]
fn resolve_gid_id_only_returns_gid() {
    let gid = resolve_gid(None, Some(0)).unwrap();
    assert_eq!(gid, Some(nix::unistd::Gid::from_raw(0)));
}

#[test]
fn resolve_gid_unknown_name_errors() {
    let err = resolve_gid(Some("_no_such_group_xyz_"), None).unwrap_err();
    assert!(err.to_string().contains("not found") || err.to_string().contains("getgrnam"));
}

#[test]
fn resolve_gid_name_and_matching_id() {
    // "root" group always has gid 0 on Linux.
    let gid = resolve_gid(Some("root"), Some(0)).unwrap();
    assert_eq!(gid, Some(nix::unistd::Gid::from_raw(0)));
}

#[test]
fn resolve_gid_name_and_mismatched_id_errors() {
    let err = resolve_gid(Some("root"), Some(9999)).unwrap_err();
    assert!(err.to_string().contains("gid") || err.to_string().contains("group-id"));
}
