//! An RFC 3339 timestamp with the exact JSON spelling the reference
//! implementation produces.
//!
//! `time.Time` marshals as RFC 3339 with nanosecond precision, trimming trailing
//! zeros from the fractional part and omitting it entirely when it is zero. Plan
//! and snapshot documents carry those strings, so the spelling is part of the
//! contract: `2026-11-01T12:10:41.5123456Z`, not `…41.512345600Z` and not
//! `…41Z`, unless the nanoseconds really are zero.
//!
//! The type is deliberately small: the kernel needs "now", "now plus fifteen
//! minutes", ordering, and the two spellings above. A calendar library would be
//! a dependency for four functions.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A point in time, always UTC.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    /// Whole seconds since the Unix epoch.
    secs: i64,
    /// The nanosecond within the second, `0..1_000_000_000`.
    nanos: u32,
}

impl Timestamp {
    /// The clock reading at this instant.
    pub fn now() -> Timestamp {
        Self::from_system_time(SystemTime::now())
    }

    /// A timestamp from Unix seconds and nanoseconds.
    pub fn from_unix(secs: i64, nanos: u32) -> Timestamp {
        Timestamp {
            secs,
            nanos: nanos.min(999_999_999),
        }
    }

    /// A timestamp from a [`SystemTime`].
    pub fn from_system_time(time: SystemTime) -> Timestamp {
        match time.duration_since(UNIX_EPOCH) {
            Ok(after) => Timestamp {
                secs: after.as_secs() as i64,
                nanos: after.subsec_nanos(),
            },
            // Before the epoch: `SystemTime` has no signed duration, so borrow a
            // second from the negative side.
            Err(before) => {
                let duration = before.duration();
                let secs = -(duration.as_secs() as i64);
                if duration.subsec_nanos() == 0 {
                    Timestamp { secs, nanos: 0 }
                } else {
                    Timestamp {
                        secs: secs - 1,
                        nanos: 1_000_000_000 - duration.subsec_nanos(),
                    }
                }
            }
        }
    }

    /// Whole seconds since the epoch.
    pub fn unix_secs(self) -> i64 {
        self.secs
    }

    /// The nanosecond within the second.
    pub fn nanos(self) -> u32 {
        self.nanos
    }

    /// Adds a duration, saturating at the ends of the representable range.
    pub fn plus(self, duration: Duration) -> Timestamp {
        let secs = self.secs.saturating_add(duration.as_secs() as i64);
        let nanos = u64::from(self.nanos) + u64::from(duration.subsec_nanos());
        if nanos >= 1_000_000_000 {
            Timestamp {
                secs: secs.saturating_add(1),
                nanos: (nanos - 1_000_000_000) as u32,
            }
        } else {
            Timestamp {
                secs,
                nanos: nanos as u32,
            }
        }
    }

    /// The UTC year.
    pub fn year(self) -> i64 {
        self.civil().0
    }

    /// The UTC month, 1-12.
    pub fn month(self) -> u32 {
        self.civil().1
    }

    /// The UTC day of the month, 1-31.
    pub fn day(self) -> u32 {
        self.civil().2
    }

    /// The UTC hour, 0-23.
    pub fn hour(self) -> u32 {
        self.civil().3
    }

    /// The UTC minute, 0-59.
    pub fn minute(self) -> u32 {
        self.civil().4
    }

    /// The UTC second, 0-59.
    pub fn second(self) -> u32 {
        self.civil().5
    }

    /// The RFC 3339 spelling with nanosecond precision, trailing zeros trimmed.
    pub fn to_rfc3339(self) -> String {
        let (year, month, day, hour, minute, second) = self.civil();
        let mut text = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}");
        if self.nanos != 0 {
            let mut fraction = format!("{:09}", self.nanos);
            while fraction.ends_with('0') {
                fraction.pop();
            }
            text.push('.');
            text.push_str(&fraction);
        }
        text.push('Z');
        text
    }

    /// The RFC 3339 spelling without a fractional part.
    ///
    /// That is Go's `time.RFC3339` layout, which the human-facing plan header
    /// prints.
    pub fn to_rfc3339_seconds(self) -> String {
        let (year, month, day, hour, minute, second) = self.civil();
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
    }

    /// Parses an RFC 3339 timestamp, accepting `Z` or a numeric offset.
    pub fn parse(text: &str) -> Option<Timestamp> {
        let bytes = text.as_bytes();
        if bytes.len() < 20 {
            return None;
        }
        let year: i64 = text.get(0..4)?.parse().ok()?;
        if bytes[4] != b'-' {
            return None;
        }
        let month: u32 = text.get(5..7)?.parse().ok()?;
        if bytes[7] != b'-' {
            return None;
        }
        let day: u32 = text.get(8..10)?.parse().ok()?;
        if bytes[10] != b'T' && bytes[10] != b't' && bytes[10] != b' ' {
            return None;
        }
        let hour: i64 = text.get(11..13)?.parse().ok()?;
        if bytes[13] != b':' {
            return None;
        }
        let minute: i64 = text.get(14..16)?.parse().ok()?;
        if bytes[16] != b':' {
            return None;
        }
        let second: i64 = text.get(17..19)?.parse().ok()?;

        let mut rest = 19;
        let mut nanos: u32 = 0;
        if bytes.get(rest) == Some(&b'.') {
            rest += 1;
            let start = rest;
            while rest < bytes.len() && bytes[rest].is_ascii_digit() {
                rest += 1;
            }
            if rest == start {
                return None;
            }
            let digits = text.get(start..rest)?;
            let mut scaled = String::with_capacity(9);
            scaled.push_str(&digits[..digits.len().min(9)]);
            while scaled.len() < 9 {
                scaled.push('0');
            }
            nanos = scaled.parse().ok()?;
        }

        let zone = text.get(rest..)?;
        let offset: i64 = match zone {
            "Z" | "z" => 0,
            _ => {
                let sign = match zone.as_bytes().first()? {
                    b'+' => 1,
                    b'-' => -1,
                    _ => return None,
                };
                if zone.len() != 6 || zone.as_bytes()[3] != b':' {
                    return None;
                }
                let hours: i64 = zone.get(1..3)?.parse().ok()?;
                let minutes: i64 = zone.get(4..6)?.parse().ok()?;
                sign * (hours * 3600 + minutes * 60)
            }
        };

        let days = days_from_civil(year, month, day)?;
        let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
        Some(Timestamp { secs, nanos })
    }

    /// The civil fields `(year, month, day, hour, minute, second)` in UTC.
    fn civil(self) -> (i64, u32, u32, u32, u32, u32) {
        let days = self.secs.div_euclid(86_400);
        let seconds_of_day = self.secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let hour = (seconds_of_day / 3600) as u32;
        let minute = ((seconds_of_day % 3600) / 60) as u32;
        let second = (seconds_of_day % 60) as u32;
        (year, month, day, hour, minute, second)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_rfc3339())
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TimestampVisitor;

        impl Visitor<'_> for TimestampVisitor {
            type Value = Timestamp;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an RFC 3339 timestamp")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Timestamp, E> {
                Timestamp::parse(value).ok_or_else(|| E::custom(format!("bad timestamp {value:?}")))
            }
        }

        deserializer.deserialize_str(TimestampVisitor)
    }
}

/// Days since the Unix epoch for a civil date, or `None` for an impossible one.
fn days_from_civil(year: i64, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

/// The civil date for a count of days since the Unix epoch.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_formats_and_parses() {
        let epoch = Timestamp::from_unix(0, 0);
        assert_eq!(epoch.to_rfc3339(), "1970-01-01T00:00:00Z");
        assert_eq!(Timestamp::parse("1970-01-01T00:00:00Z"), Some(epoch));
    }

    /// The reference implementation trims trailing zeros from the fraction.
    #[test]
    fn the_fraction_is_trimmed_like_the_reference_implementation() {
        let stamped = Timestamp::from_unix(1_790_000_000, 512_345_600);
        assert_eq!(stamped.to_rfc3339(), "2026-09-21T14:13:20.5123456Z");

        let whole = Timestamp::from_unix(1_790_000_000, 0);
        assert_eq!(whole.to_rfc3339(), "2026-09-21T14:13:20Z");
    }

    #[test]
    fn a_timestamp_round_trips_through_json() {
        let stamped = Timestamp::parse("2026-11-01T12:10:41.5123456Z").expect("parse");
        let json = serde_json::to_string(&stamped).expect("serialize");
        assert_eq!(json, "\"2026-11-01T12:10:41.5123456Z\"");
        let back: Timestamp = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, stamped);
    }

    #[test]
    fn an_offset_is_normalised_to_utc() {
        let parsed = Timestamp::parse("2026-11-01T20:10:41+08:00").expect("parse");
        assert_eq!(parsed.to_rfc3339(), "2026-11-01T12:10:41Z");
    }

    #[test]
    fn fifteen_minutes_later_is_later() {
        let now = Timestamp::parse("2026-11-01T12:10:41.5123456Z").expect("parse");
        let later = now.plus(Duration::from_secs(15 * 60));
        assert_eq!(later.to_rfc3339(), "2026-11-01T12:25:41.5123456Z");
        assert!(later > now);
    }

    #[test]
    fn ordering_is_by_instant_then_nanoseconds() {
        let first = Timestamp::from_unix(100, 1);
        let second = Timestamp::from_unix(100, 2);
        let third = Timestamp::from_unix(101, 0);
        assert!(first < second && second < third);
    }

    #[test]
    fn a_leap_day_survives_the_round_trip() {
        for text in [
            "2024-02-29T23:59:59Z",
            "2000-02-29T00:00:00Z",
            "1999-12-31T23:59:59.999999999Z",
        ] {
            let parsed = Timestamp::parse(text).expect("parse");
            assert_eq!(parsed.to_rfc3339(), text);
        }
    }

    #[test]
    fn nonsense_is_rejected() {
        assert_eq!(Timestamp::parse(""), None);
        assert_eq!(Timestamp::parse("not a time"), None);
        assert_eq!(Timestamp::parse("2026-13-01T00:00:00Z"), None);
    }
}
