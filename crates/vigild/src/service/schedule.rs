// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Cron schedules for `type: oneshot` services (`schedule`, `schedule-tz`).
//!
//! Patterns have 5 fields (minute hour day-of-month month day-of-week) or 6
//! with seconds first, as in Vixie cron (`dom` and `dow` are OR-ed). Time
//! zones come from the system time zone database (`/usr/share/zoneinfo`);
//! without `schedule-tz` the schedule runs in UTC. DST transitions are
//! handled by croner on `jiff::Zoned` times.

use anyhow::Context;
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use jiff::Timestamp;
use jiff::tz::TimeZone;

#[derive(Debug, Clone)]
pub struct Schedule {
    cron: Cron,
    tz: TimeZone,
}

impl Schedule {
    pub fn parse(pattern: &str, tz: Option<&str>) -> anyhow::Result<Self> {
        let cron = CronParser::builder()
            .seconds(Seconds::Optional)
            .year(Year::Disallowed)
            .build()
            .parse(pattern)
            .map_err(|e| anyhow::anyhow!("invalid schedule {pattern:?}: {e}"))?;
        let tz = match tz {
            None => TimeZone::UTC,
            Some(name) => TimeZone::get(name).with_context(|| {
                format!(
                    "unknown schedule-tz {name:?} (is the time zone database, \
                     e.g. the tzdata package, installed?)"
                )
            })?,
        };
        Ok(Schedule { cron, tz })
    }

    /// First scheduled time strictly after `after`.
    pub fn next_after(&self, after: Timestamp) -> anyhow::Result<Timestamp> {
        let zoned = after.to_zoned(self.tz.clone());
        let next = self
            .cron
            .find_next_occurrence(&zoned, false)
            .map_err(|e| anyhow::anyhow!("no next run: {e}"))?;
        Ok(next.timestamp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn five_fields_run_in_utc_by_default() {
        let s = Schedule::parse("*/5 * * * *", None).unwrap();
        assert_eq!(
            s.next_after(ts("2026-11-03T10:02:30Z")).unwrap(),
            ts("2026-11-03T10:05:00Z")
        );
        // strictly after
        assert_eq!(
            s.next_after(ts("2026-11-03T10:05:00Z")).unwrap(),
            ts("2026-11-03T10:10:00Z")
        );
    }

    #[test]
    fn six_fields_have_seconds_first() {
        let s = Schedule::parse("*/10 * * * * *", None).unwrap();
        assert_eq!(
            s.next_after(ts("2026-11-03T10:00:01Z")).unwrap(),
            ts("2026-11-03T10:00:10Z")
        );
    }

    #[test]
    fn cron_extensions() {
        let after = ts("2026-11-03T00:00:00Z");
        let next = |p| Schedule::parse(p, None).unwrap().next_after(after).unwrap();
        assert_eq!(next("0 0 L * *"), ts("2026-11-30T00:00:00Z")); // last day
        assert_eq!(next("0 0 * * FRI#2"), ts("2026-11-13T00:00:00Z")); // 2nd Friday
        assert_eq!(next("0 0 15W * *"), ts("2026-11-16T00:00:00Z")); // weekday nearest the 15th
    }

    #[test]
    fn invalid_patterns_are_rejected() {
        for p in [
            "",
            "* * * *",
            "61 * * * *",
            "* * * * * * *",
            "0 0 3 * * * 2027",
            "nonsense",
        ] {
            assert!(Schedule::parse(p, None).is_err(), "{p:?}");
        }
    }

    #[test]
    fn unknown_time_zone_is_rejected() {
        let err = Schedule::parse("0 3 * * *", Some("Mars/Olympus"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown schedule-tz"), "{err}");
    }

    #[test]
    fn time_zone_and_dst() {
        // Needs the system tzdb; skip where it is missing (minimal images).
        let Ok(s) = Schedule::parse("30 2 * * *", Some("Europe/Vienna")) else {
            eprintln!("skipped: no time zone database");
            return;
        };
        // Winter time (UTC+1): 02:30 local = 01:30 UTC.
        assert_eq!(
            s.next_after(ts("2026-11-03T00:00:00Z")).unwrap(),
            ts("2026-11-03T01:30:00Z")
        );
        // 2026-03-29 (spring forward 02:00 -> 03:00): 02:30 does not exist,
        // the job runs once at 03:00 local.
        assert_eq!(
            s.next_after(ts("2026-03-28T23:00:00Z")).unwrap(),
            ts("2026-03-29T01:00:00Z")
        );
        // 2026-10-25 (fall back 03:00 -> 02:00): 02:30 exists twice, the job
        // runs once (first occurrence, still summer time), then next day.
        let first = s.next_after(ts("2026-10-24T23:00:00Z")).unwrap();
        assert_eq!(first, ts("2026-10-25T00:30:00Z"));
        assert_eq!(s.next_after(first).unwrap(), ts("2026-10-26T01:30:00Z"));
    }
}
