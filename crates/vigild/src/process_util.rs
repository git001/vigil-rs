// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

/// Shared process-launching helpers used by both service.rs and check.rs.
use anyhow::Context;
use nix::unistd::{Gid, Uid};

// ---------------------------------------------------------------------------
// Command parsing
// ---------------------------------------------------------------------------

/// Parse a pebble-style command string into argv.
///
/// Supports quoted strings and the default-args bracket syntax:
///   `"/usr/bin/foo arg1 [ --default-port 8080 ]"`
///
/// The `[ ... ]` section contains default arguments that are always included
/// (they can be overridden in a future `--args` extension; for now they are
/// unconditionally appended).
pub fn parse_command(s: &str) -> anyhow::Result<Vec<String>> {
    let s = s.trim();
    // Look for " [ " ... " ]" bracket section, but only outside quoted strings
    // so that shell `if [ ... ]` constructs inside single-quoted arguments are
    // not mistaken for the default-args syntax.
    if let Some(open) = find_unquoted(s, " [ ") {
        let close = s.rfind(" ]")
            .ok_or_else(|| anyhow::anyhow!("unmatched '[' in command: {:?}", s))?;
        if close < open {
            anyhow::bail!("malformed default-args syntax in command: {:?}", s);
        }
        let base = &s[..open];
        let defaults = &s[open + 3..close];
        let mut args = shell_words(base)?;
        args.extend(shell_words(defaults)?);
        Ok(args)
    } else {
        shell_words(s)
    }
}

/// Find the byte-position of `needle` in `s` while skipping over single- and
/// double-quoted substrings (and backslash escapes outside single quotes).
/// Returns `None` if `needle` only appears inside a quoted region.
fn find_unquoted(s: &str, needle: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let nb = needle.as_bytes();
    let sb = s.as_bytes();

    let mut i = 0;
    while i < sb.len() {
        let c = sb[i] as char;
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        match c {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            _ if !in_single && !in_double => {
                if sb[i..].starts_with(nb) {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Minimal shell-word splitter: handles single/double quotes and backslash
/// escapes. Does NOT perform variable expansion.
pub fn shell_words(s: &str) -> anyhow::Result<Vec<String>> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for c in s.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ' ' | '\t' | '\n' | '\r' if !in_single && !in_double => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }

    if in_single || in_double {
        anyhow::bail!("unterminated quote in command: {:?}", s);
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

// ---------------------------------------------------------------------------
// UID / GID resolution
// ---------------------------------------------------------------------------

/// Resolve the effective UID from an optional username and/or numeric user-id.
///
/// - Both set: look up username, verify it matches user-id.
/// - Only name: look up by name.
/// - Only id: use directly.
/// - Neither: `None` (keep current user).
pub fn resolve_uid(user: Option<&str>, user_id: Option<u32>) -> anyhow::Result<Option<Uid>> {
    match (user, user_id) {
        (Some(name), Some(id)) => {
            let uid = lookup_uid(name)?;
            anyhow::ensure!(
                uid.as_raw() == id,
                "user {:?} has uid {} but user-id {} was specified",
                name,
                uid.as_raw(),
                id
            );
            Ok(Some(uid))
        }
        (Some(name), None) => Ok(Some(lookup_uid(name)?)),
        (None, Some(id)) => Ok(Some(Uid::from_raw(id))),
        (None, None) => Ok(None),
    }
}

/// Resolve the effective GID from an optional group name and/or numeric group-id.
pub fn resolve_gid(group: Option<&str>, group_id: Option<u32>) -> anyhow::Result<Option<Gid>> {
    match (group, group_id) {
        (Some(name), Some(id)) => {
            let gid = lookup_gid(name)?;
            anyhow::ensure!(
                gid.as_raw() == id,
                "group {:?} has gid {} but group-id {} was specified",
                name,
                gid.as_raw(),
                id
            );
            Ok(Some(gid))
        }
        (Some(name), None) => Ok(Some(lookup_gid(name)?)),
        (None, Some(id)) => Ok(Some(Gid::from_raw(id))),
        (None, None) => Ok(None),
    }
}

fn lookup_uid(name: &str) -> anyhow::Result<Uid> {
    nix::unistd::User::from_name(name)
        .with_context(|| format!("getpwnam({:?})", name))?
        .map(|u| u.uid)
        .ok_or_else(|| anyhow::anyhow!("user {:?} not found", name))
}

fn lookup_gid(name: &str) -> anyhow::Result<Gid> {
    nix::unistd::Group::from_name(name)
        .with_context(|| format!("getgrnam({:?})", name))?
        .map(|g| g.gid)
        .ok_or_else(|| anyhow::anyhow!("group {:?} not found", name))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        assert_eq!(parse_command("/bin/foo bar baz").unwrap(), vec!["/bin/foo", "bar", "baz"]);
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
        assert_eq!(
            parse_command("/bin/foo [ ]").unwrap(),
            vec!["/bin/foo"]
        );
    }

    #[test]
    fn parse_shell_if_bracket_in_quoted_arg() {
        // `[ ... ]` inside a single-quoted sh -c argument must NOT be treated
        // as the default-args bracket syntax.
        let cmd = r#"sh -c 'if [ $((i % 7)) -eq 0 ]; then echo err; fi'"#;
        let argv = parse_command(cmd).unwrap();
        assert_eq!(argv[0], "sh");
        assert_eq!(argv[1], "-c");
        assert!(argv[2].contains("if ["), "shell script preserved: {:?}", argv[2]);
        assert_eq!(argv.len(), 3);
    }

    #[test]
    fn parse_newlines_as_whitespace() {
        // Newlines outside quotes (e.g. from YAML folded scalars) are treated
        // as word separators, not embedded in tokens.
        let cmd = "sh -c 'script'\n";
        assert_eq!(parse_command(cmd).unwrap(), vec!["sh", "-c", "script"]);
    }
}
