// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

/// Shortest allowed polling interval: check `period`, `watch-files`
/// `interval` and `--watch-layers`. Every round runs a probe or reads files,
/// which must not flood the pod, a checked endpoint or a file server (NFS).
pub const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// Longest allowed polling interval (see [`MIN_INTERVAL`]).
pub const MAX_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Keep an already validated interval within the bounds anyway, so a value
/// that bypassed validation can never poll in a tight loop.
pub fn clamp_interval(d: std::time::Duration) -> std::time::Duration {
    d.clamp(MIN_INTERVAL, MAX_INTERVAL)
}

/// Parse a polling interval and require it to be within [`MIN_INTERVAL`]
/// and [`MAX_INTERVAL`]. The error names the value, the caller adds the
/// field.
pub fn parse_interval(s: &str) -> Result<std::time::Duration, String> {
    let d = parse_duration(s).map_err(|e| format!("{s:?} is not a duration: {e}"))?;
    if !(MIN_INTERVAL..=MAX_INTERVAL).contains(&d) {
        return Err(format!("{s:?} must be between 1s and 24h"));
    }
    Ok(d)
}

/// Parse an optional duration setting. An invalid value is an error that
/// names the setting (`kill-delay "10 sec" is not a duration: …`), never a
/// silent fallback to the default.
pub fn parse_field(
    field: &str,
    value: Option<&str>,
) -> anyhow::Result<Option<std::time::Duration>> {
    value
        .map(|s| {
            parse_duration(s).map_err(|e| anyhow::anyhow!("{field} {s:?} is not a duration: {e}"))
        })
        .transpose()
}

/// Parse a human-readable duration string into `std::time::Duration`.
///
/// Supported suffixes: ms, s, m, h
/// Examples: "500ms", "5s", "1m", "2h", "1m30s"
///
/// Multiple components can be chained: "1m30s", "2h5m"
pub fn parse_duration(s: &str) -> Result<std::time::Duration, String> {
    if s.is_empty() {
        return Err("empty duration string".into());
    }

    let mut total_ms: u64 = 0;
    let mut rest = s;

    while !rest.is_empty() {
        // Read digits
        let num_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if num_end == 0 {
            return Err(format!("expected digit in duration: {:?}", rest));
        }
        let num: u64 = rest[..num_end]
            .parse()
            .map_err(|_| format!("number overflow in duration: {:?}", s))?;
        rest = &rest[num_end..];

        // Read suffix
        let (mult, suffix_len) = if rest.starts_with("ms") {
            (1u64, 2)
        } else if rest.starts_with('s') {
            (1_000, 1)
        } else if rest.starts_with('m') {
            (60_000, 1)
        } else if rest.starts_with('h') {
            (3_600_000, 1)
        } else {
            return Err(format!("unknown duration suffix in {:?}", s));
        };

        total_ms = total_ms
            .checked_add(
                num.checked_mul(mult)
                    .ok_or_else(|| format!("overflow: {:?}", s))?,
            )
            .ok_or_else(|| format!("overflow: {:?}", s))?;
        rest = &rest[suffix_len..];
    }

    Ok(std::time::Duration::from_millis(total_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn basic() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("5s").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("2h5m").unwrap(), Duration::from_secs(7500));
    }

    #[test]
    fn errors() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn interval_is_bounded() {
        assert_eq!(parse_interval("1s"), Ok(MIN_INTERVAL));
        assert_eq!(parse_interval("24h"), Ok(MAX_INTERVAL));
        assert_eq!(
            parse_interval("30s"),
            Ok(std::time::Duration::from_secs(30))
        );
        for s in ["0s", "999ms", "25h"] {
            let e = parse_interval(s).unwrap_err();
            assert!(e.ends_with("must be between 1s and 24h"), "{s}: {e}");
        }
        assert!(
            parse_interval("soon")
                .unwrap_err()
                .starts_with("\"soon\" is not a duration")
        );
    }
}
