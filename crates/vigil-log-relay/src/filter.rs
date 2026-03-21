//! Line-level include/exclude regex filter — shared by all source modes.
//!
//! Multiple patterns per flag are OR-combined:
//! - `--include REGEX` (repeatable): forward line if **any** pattern matches
//! - `--exclude REGEX` (repeatable): drop line if **any** pattern matches (after --include)
//! - No patterns set: all lines pass

use anyhow::{Context, Result};
use regex::Regex;

/// Decides whether a log line should be forwarded to the TCP sink.
#[derive(Debug)]
pub struct LineFilter {
    include: Vec<Regex>,
    exclude: Vec<Regex>,
}

impl LineFilter {
    /// Build a filter from string slices — convenience for tests.
    #[cfg(test)]
    pub fn from_strs(include: &[&str], exclude: &[&str]) -> Self {
        let to_owned = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        Self::new(&to_owned(include), &to_owned(exclude)).expect("test regex invalid")
    }

    /// Build a filter from lists of pattern strings.
    pub fn new(include: &[String], exclude: &[String]) -> Result<Self> {
        Ok(Self {
            include: include
                .iter()
                .map(|p| Regex::new(p).with_context(|| format!("invalid --include regex: {p}")))
                .collect::<Result<Vec<_>>>()?,
            exclude: exclude
                .iter()
                .map(|p| Regex::new(p).with_context(|| format!("invalid --exclude regex: {p}")))
                .collect::<Result<Vec<_>>>()?,
        })
    }

    /// Returns `true` if `line` should be forwarded.
    #[allow(clippy::inline_always)]
    #[inline]
    pub fn allow(&self, line: &str) -> bool {
        if !self.include.is_empty() && !self.include.iter().any(|re| re.is_match(line)) {
            return false;
        }
        if self.exclude.iter().any(|re| re.is_match(line)) {
            return false;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- no filters ---

    #[test]
    fn no_filters_passes_everything() {
        let f = LineFilter::from_strs(&[], &[]);
        assert!(f.allow("anything goes"));
        assert!(f.allow(""));
    }

    // --- include only ---

    #[test]
    fn include_passes_matching_lines() {
        let f = LineFilter::from_strs(&["ERROR"], &[]);
        assert!(f.allow("ERROR: disk full"));
        assert!(!f.allow("INFO: all good"));
    }

    #[test]
    fn multiple_includes_are_or_combined() {
        let f = LineFilter::from_strs(&["ERROR", "WARN"], &[]);
        assert!(f.allow("ERROR: disk full"));
        assert!(f.allow("WARN: low memory"));
        assert!(!f.allow("INFO: all good"));
    }

    #[test]
    fn include_regex_is_substring_match() {
        let f = LineFilter::from_strs(&["err"], &[]);
        assert!(f.allow("unexpected error occurred"));
        assert!(!f.allow("all systems nominal"));
    }

    // --- exclude only ---

    #[test]
    fn exclude_drops_matching_lines() {
        let f = LineFilter::from_strs(&[], &["healthz"]);
        assert!(!f.allow("GET /healthz 200"));
        assert!(f.allow("GET /api/users 200"));
    }

    #[test]
    fn multiple_excludes_are_or_combined() {
        let f = LineFilter::from_strs(&[], &["healthz", "readyz"]);
        assert!(!f.allow("GET /healthz 200"));
        assert!(!f.allow("GET /readyz 200"));
        assert!(f.allow("GET /api/data 200"));
    }

    // --- include + exclude ---

    #[test]
    fn exclude_applied_after_include() {
        // include ERROR, but exclude healthcheck errors
        let f = LineFilter::from_strs(&["ERROR"], &["GET /healthz"]);
        assert!(f.allow("ERROR: db timeout"));
        assert!(!f.allow("ERROR: GET /healthz failed")); // matches exclude
        assert!(!f.allow("INFO: normal log")); // doesn't match include
    }

    // --- invalid regex ---

    #[test]
    fn invalid_include_regex_returns_error() {
        let result = LineFilter::new(&["[invalid".to_string()], &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--include regex"));
    }

    #[test]
    fn invalid_exclude_regex_returns_error() {
        let result = LineFilter::new(&[], &["[invalid".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--exclude regex"));
    }
}
