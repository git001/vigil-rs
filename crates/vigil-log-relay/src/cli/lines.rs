//! How vigil-log-relay cuts and joins log lines.

use std::time::Duration;

use clap::Args;
use vigil_types::lines::{DEFAULT_MAX_LINE, Rule, parse_timeout};
use vigil_types::plan::{LogsMultiline, MultilinePreset};

/// Smallest allowed `--max-line-bytes`.
pub const MIN_MAX_LINE: usize = 1024;
/// Largest allowed `--max-line-bytes`.
pub const MAX_MAX_LINE: usize = 64 * 1024 * 1024;

#[derive(Args, Debug)]
pub struct Lines {
    // ---- Lines ------------------------------------------------------------
    /// Longest log message in bytes (1024 to 67108864), as vigild's
    /// --log-max-line-bytes.
    ///
    /// A longer pod log line (or a multi-line entry that would grow past
    /// it) is forwarded in parts. A record from vigild (ndjson or SSE line)
    /// may be up to 6 times as long plus 4 KiB, for JSON escaping; a longer
    /// one is forwarded in parts too, which are no valid JSON. Nothing is
    /// dropped; each split line is logged as a warning.
    #[arg(
        long,
        env = "MAX_LINE_BYTES",
        value_name = "BYTES",
        default_value_t = DEFAULT_MAX_LINE,
        value_parser = parse_max_line,
        help_heading = "Lines"
    )]
    pub max_line_bytes: usize,

    /// Join the lines of a stack trace into one record: java, python or
    /// indent.
    ///
    /// Records from vigild are joined per service and stream on their
    /// message; other lines as text.
    #[arg(
        long,
        env = "MULTILINE",
        value_name = "PRESET",
        value_parser = parse_preset,
        conflicts_with_all = ["multiline_start", "multiline_continue"],
        help_heading = "Lines"
    )]
    pub multiline: Option<MultilinePreset>,

    /// Join lines: a line (message) that matches this regex starts a new
    /// record, every other one belongs to the record before it.
    #[arg(
        long,
        env = "MULTILINE_START",
        value_name = "REGEX",
        conflicts_with = "multiline_continue",
        help_heading = "Lines"
    )]
    pub multiline_start: Option<String>,

    /// Join lines: a line (message) that matches this regex belongs to the
    /// record before it.
    #[arg(
        long,
        env = "MULTILINE_CONTINUE",
        value_name = "REGEX",
        help_heading = "Lines"
    )]
    pub multiline_continue: Option<String>,

    /// How long a joined record waits for a further line (10ms to 10s).
    #[arg(
        long,
        env = "MULTILINE_TIMEOUT",
        value_name = "DURATION",
        default_value = "200ms",
        value_parser = parse_timeout,
        help_heading = "Lines"
    )]
    pub multiline_timeout: Duration,
}

impl Lines {
    /// The multi-line rule, if one of the three options is set.
    pub fn multiline_rule(&self) -> Result<Option<Rule>, String> {
        let config = LogsMultiline {
            preset: self.multiline,
            start: self.multiline_start.clone(),
            continue_: self.multiline_continue.clone(),
            timeout: None,
        };
        if config == LogsMultiline::default() {
            return Ok(None);
        }
        Rule::from_config(&config)
            .map(Some)
            .map_err(|e| e.replace("logs-multiline: ", "--multiline-"))
    }
}

/// `--max-line-bytes`: bytes, within [`MIN_MAX_LINE`] and [`MAX_MAX_LINE`].
fn parse_max_line(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|_| format!("{s:?} is not a number of bytes"))?;
    if !(MIN_MAX_LINE..=MAX_MAX_LINE).contains(&n) {
        return Err(format!(
            "{n} must be between {MIN_MAX_LINE} and {MAX_MAX_LINE} bytes"
        ));
    }
    Ok(n)
}

/// `--multiline`: the name of a preset.
fn parse_preset(s: &str) -> Result<MultilinePreset, String> {
    match s {
        "java" => Ok(MultilinePreset::Java),
        "python" => Ok(MultilinePreset::Python),
        "indent" => Ok(MultilinePreset::Indent),
        _ => Err(format!("{s:?} is no preset: java, python or indent")),
    }
}

/// The longest record from vigild (ndjson or SSE line) for a message of
/// at most `max` bytes: JSON escaping makes a byte up to 6 (`\u0000`), and
/// the other fields of the record need room too.
pub fn record_limit(max: usize) -> usize {
    max.saturating_mul(6).saturating_add(4096)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_line_and_preset_are_checked() {
        assert_eq!(parse_max_line("1024"), Ok(1024));
        assert!(
            parse_max_line("67108865")
                .unwrap_err()
                .ends_with("must be between 1024 and 67108864 bytes")
        );
        assert_eq!(parse_preset("python"), Ok(MultilinePreset::Python));
        assert_eq!(
            parse_preset("ruby").unwrap_err(),
            "\"ruby\" is no preset: java, python or indent"
        );
        assert_eq!(record_limit(1024), 6 * 1024 + 4096);
    }
}
