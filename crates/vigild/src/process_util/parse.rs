// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

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
        let close = s
            .rfind(" ]")
            .ok_or_else(|| anyhow::anyhow!("unmatched '[' in command: {:?}", s))?;
        if close < open {
            anyhow::bail!("malformed default-args syntax in command: {:?}", s);
        }
        let base = &s[..open];
        let defaults_start = open + 3;
        let defaults = if defaults_start <= close {
            &s[defaults_start..close]
        } else {
            ""
        };
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
pub(super) fn find_unquoted(s: &str, needle: &str) -> Option<usize> {
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
