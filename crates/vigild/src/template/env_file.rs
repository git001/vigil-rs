// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `env-file`: files with one `KEY=value` per line, as written by the Vault
//! Agent, docker `--env-file` or systemd `EnvironmentFile=`.
//!
//! ```text
//! # comment
//! DB_USER=app
//! export DB_PASSWORD="s3cr\"et"     # `export ` is optional
//! TOKEN='literal $value'
//! ```
//!
//! - Unquoted values run to the end of the line (trailing whitespace is
//!   removed; `#` is part of the value).
//! - `'…'` is literal; `"…"` understands `\"`, `\\`, `\n`, `\t`, `\$`.
//!   After the closing quote only whitespace or a `# comment` may follow.
//! - No variable expansion, no multi-line values.
//!
//! Errors name the file and line, never a value.

use indexmap::IndexMap;

use super::file::read_text;

/// Largest env file read.
const MAX_ENV_FILE: u64 = 1024 * 1024;

/// `-/path` → (`/path`, optional).
pub fn split_optional(entry: &str) -> (&str, bool) {
    match entry.strip_prefix('-') {
        Some(p) => (p, true),
        None => (entry, false),
    }
}

/// Check an `env-file` entry without reading it (at load and replan).
pub fn check_entry(entry: &str) -> anyhow::Result<()> {
    let (path, _) = split_optional(entry);
    anyhow::ensure!(
        path.starts_with('/'),
        "env-file {entry:?}: path must be absolute"
    );
    Ok(())
}

/// Read all `entries` in order into `out` (later files override earlier).
pub fn load(entries: &[String], out: &mut IndexMap<String, String>) -> anyhow::Result<()> {
    for entry in entries {
        let (path, optional) = split_optional(entry);
        match read_text(path, MAX_ENV_FILE).map_err(|e| anyhow::anyhow!("env-file {e}"))? {
            Some(text) => {
                parse_into(&text, out).map_err(|e| anyhow::anyhow!("env-file {path:?}: {e}"))?
            }
            None if optional => {}
            None => anyhow::bail!("env-file {path:?}: No such file or directory"),
        }
    }
    Ok(())
}

fn valid_key(k: &str) -> bool {
    let mut chars = k.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) fn parse_into(text: &str, out: &mut IndexMap<String, String>) -> Result<(), String> {
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line
            .strip_prefix("export")
            .filter(|rest| rest.starts_with([' ', '\t']))
            .map_or(line, str::trim_start);
        let (key, raw) = line
            .split_once('=')
            .ok_or_else(|| format!("line {n}: expected KEY=value"))?;
        let key = key.trim_end();
        if !valid_key(key) {
            return Err(format!("line {n}: invalid variable name"));
        }
        let value = parse_value(raw.trim_start()).map_err(|e| format!("line {n} ({key}): {e}"))?;
        out.insert(key.to_owned(), value);
    }
    Ok(())
}

fn parse_value(raw: &str) -> Result<String, &'static str> {
    let (value, rest) = match raw.chars().next() {
        Some('\'') => {
            let end = raw[1..].find('\'').ok_or("unterminated single quote")?;
            (raw[1..1 + end].to_owned(), &raw[end + 2..])
        }
        Some('"') => {
            let mut v = String::new();
            let mut chars = raw[1..].char_indices();
            let end = loop {
                match chars.next() {
                    None => return Err("unterminated double quote"),
                    Some((i, '"')) => break i + 2,
                    Some((_, '\\')) => match chars.next() {
                        Some((_, 'n')) => v.push('\n'),
                        Some((_, 't')) => v.push('\t'),
                        Some((_, c @ ('"' | '\\' | '$'))) => v.push(c),
                        Some((_, c)) => {
                            v.push('\\');
                            v.push(c);
                        }
                        None => return Err("unterminated double quote"),
                    },
                    Some((_, c)) => v.push(c),
                }
            };
            (v, &raw[end..])
        }
        _ => return Ok(raw.trim_end().to_owned()),
    };
    let rest = rest.trim_start();
    if rest.is_empty() || rest.starts_with('#') {
        Ok(value)
    } else {
        Err("unexpected text after the closing quote")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Vec<(String, String)>, String> {
        let mut m = IndexMap::new();
        parse_into(text, &mut m)?;
        Ok(m.into_iter().collect())
    }

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parses_common_formats() {
        let text = r#"
# Vault Agent template output
export DB_USER=app
export DB_PASSWORD="p@ss \"w0rd\" \$x\n"   # quoted
TOKEN='literal $HOME \n'
  SPACED = value with spaces   
HASH=abc#def
EMPTY=
QUOTED_EMPTY=""
exportX=1
DB_USER=override
"#;
        assert_eq!(
            parse(text).unwrap(),
            kv(&[
                ("DB_USER", "override"),
                ("DB_PASSWORD", "p@ss \"w0rd\" $x\n"),
                ("TOKEN", "literal $HOME \\n"),
                ("SPACED", "value with spaces"),
                ("HASH", "abc#def"),
                ("EMPTY", ""),
                ("QUOTED_EMPTY", ""),
                ("exportX", "1"),
            ])
        );
        assert_eq!(
            parse("A=1\r\nB=2\r\n").unwrap(),
            kv(&[("A", "1"), ("B", "2")])
        );
    }

    #[test]
    fn errors_name_the_line_but_not_the_value() {
        for (text, want) in [
            ("A=1\nno equals sign here\n", "line 2: expected KEY=value"),
            ("1A=x\n", "line 1: invalid variable name"),
            ("A-B=x\n", "invalid variable name"),
            ("PW=\"s3cret\n", "line 1 (PW): unterminated double quote"),
            ("PW='s3cret\n", "unterminated single quote"),
            ("PW=\"s3cret\" trailing\n", "after the closing quote"),
        ] {
            let err = parse(text).unwrap_err();
            assert!(err.contains(want), "{text:?}: {err}");
            assert!(!err.contains("s3cret"), "{err}");
        }
    }

    #[test]
    fn load_reads_files_in_order_and_handles_optional_ones() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.env");
        let b = dir.path().join("b.env");
        std::fs::write(&a, "X=1\nY=1\n").unwrap();
        std::fs::write(&b, "Y=2\n").unwrap();
        let missing = dir.path().join("missing.env");

        let mut m = IndexMap::new();
        load(
            &[
                a.display().to_string(),
                b.display().to_string(),
                format!("-{}", missing.display()),
            ],
            &mut m,
        )
        .unwrap();
        assert_eq!(m["X"], "1");
        assert_eq!(m["Y"], "2");

        let err = load(&[missing.display().to_string()], &mut m).unwrap_err();
        assert!(err.to_string().contains("No such file"), "{err}");
        let err = load(&[dir.path().display().to_string()], &mut m).unwrap_err();
        assert!(err.to_string().contains("not a regular file"), "{err}");

        assert!(check_entry("/x").is_ok());
        assert!(check_entry("-/x").is_ok());
        assert!(check_entry("x.env").is_err());
        assert!(check_entry("-x.env").is_err());
    }
}
