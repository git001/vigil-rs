// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

pub use vigil_types::duration::parse_duration;

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

#[cfg(test)]
mod tests {
    use super::*;

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
